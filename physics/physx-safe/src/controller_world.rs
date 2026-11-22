use glam::Vec3;
use physx::{
    controller::{Controller, PxCapsuleController, PxCapsuleControllerDesc},
    controller_manager::ControllerManager,
    math::{PxExtendedVec3, PxVec3},
    scene::Scene,
    traits::Class,
};

use crate::{
    CapsuleController, CapsuleControllerDesc, ControllerHandle, ControllerMoveResult, PhysicsError,
    PhysicsWorld, body::allocate_body_handle, world::ControllerEntry,
};

impl PhysicsWorld {
    pub fn create_capsule_controller(
        &mut self,
        descriptor: CapsuleControllerDesc,
    ) -> Result<CapsuleController, PhysicsError> {
        descriptor.validate()?;
        let handle = ControllerHandle(allocate_body_handle()?);
        let mut material = self.create_material(descriptor.material)?;
        let position = descriptor.position;
        let desc = PxCapsuleControllerDesc::new(
            descriptor.height,
            descriptor.radius,
            descriptor.step_offset,
            &mut *material,
            handle,
            (position.x as f64, position.y as f64, position.z as f64).into(),
        )
        .ok_or(PhysicsError::NativeObjectCreationFailed)?;
        if self.controllers.is_none() {
            self.controllers = Some(
                self.scene
                    .create_controller_manager::<PxCapsuleController<ControllerHandle>>(false)
                    .ok_or(PhysicsError::NativeObjectCreationFailed)?,
            );
        }
        self.controllers
            .as_mut()
            .expect("controller manager just initialized")
            .create_controller(desc)
            .ok_or(PhysicsError::NativeObjectCreationFailed)?;
        let collider = CapsuleController { handle, descriptor };
        let _previous = self.controller_materials.insert(
            handle,
            ControllerEntry {
                collider,
                _material: material,
            },
        );
        crate::invariant_assert!(_previous.is_none(), "controller handles must be unique");
        Ok(collider)
    }

    pub fn capsule_controller(&self, handle: ControllerHandle) -> Option<CapsuleController> {
        self.controller_materials
            .get(&handle)
            .map(|entry| entry.collider)
    }

    fn controller_mut(
        &mut self,
        handle: ControllerHandle,
    ) -> Result<&mut PxCapsuleController<ControllerHandle>, PhysicsError> {
        if !self.controller_materials.contains_key(&handle) {
            return Err(PhysicsError::ControllerNotFound);
        }
        let manager = self.controllers.as_mut();
        crate::invariant_assert!(
            manager.is_some(),
            "registered controller must have a manager"
        );
        let manager = manager.ok_or(PhysicsError::ControllerNotFound)?;
        let controller = manager
            .get_controllers()
            .into_iter()
            .find(|controller| *controller.get_user_data() == handle);
        crate::invariant_assert!(
            controller.is_some(),
            "registered controller must exist in its manager"
        );
        controller.ok_or(PhysicsError::ControllerNotFound)
    }

    pub fn controller_position(&mut self, handle: ControllerHandle) -> Result<Vec3, PhysicsError> {
        let position = self.controller_mut(handle)?.get_position();
        Ok(Vec3::new(position.x(), position.y(), position.z()))
    }

    pub fn set_controller_position(
        &mut self,
        handle: ControllerHandle,
        position: Vec3,
    ) -> Result<(), PhysicsError> {
        if !position.is_finite() {
            return Err(PhysicsError::InvalidPose);
        }
        let controller = self.controller_mut(handle)?;
        let position: PxExtendedVec3 =
            (position.x as f64, position.y as f64, position.z as f64).into();
        // The native API copies the position during this call.
        if unsafe {
            physx_sys::PxController_setPosition_mut(controller.as_mut_ptr(), position.as_ptr())
        } {
            Ok(())
        } else {
            Err(PhysicsError::ControllerPositionFailed)
        }
    }

    /// Performs PhysX collide-and-slide movement. Gravity must be included in displacement by the caller.
    pub fn move_controller(
        &mut self,
        handle: ControllerHandle,
        displacement: Vec3,
        minimum_distance: f32,
        elapsed_time: f32,
    ) -> Result<ControllerMoveResult, PhysicsError> {
        if !displacement.is_finite() || !minimum_distance.is_finite() || minimum_distance < 0.0 {
            return Err(PhysicsError::InvalidControllerMovement);
        }
        if !elapsed_time.is_finite() || elapsed_time <= 0.0 {
            return Err(PhysicsError::InvalidTimeStep);
        }
        let controller = self.controller_mut(handle)?;
        let displacement = PxVec3::new(displacement.x, displacement.y, displacement.z);
        // The filters and displacement live for this blocking call; no callbacks or obstacles are supplied.
        let flags = unsafe {
            let filters = physx_sys::PxControllerFilters_new(
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
            physx_sys::PxController_move_mut(
                controller.as_mut_ptr(),
                displacement.as_ptr(),
                minimum_distance,
                elapsed_time,
                &filters,
                std::ptr::null(),
            )
        };
        Ok(ControllerMoveResult {
            sides: flags.contains(physx_sys::PxControllerCollisionFlags::CollisionSides),
            up: flags.contains(physx_sys::PxControllerCollisionFlags::CollisionUp),
            down: flags.contains(physx_sys::PxControllerCollisionFlags::CollisionDown),
        })
    }

    pub fn remove_controller(&mut self, handle: ControllerHandle) -> Result<(), PhysicsError> {
        let controller = self.controller_mut(handle)?;
        // The manager owns the controller; releasing it unregisters it from the manager and scene.
        unsafe { std::ptr::drop_in_place(controller as *mut _) };
        self.controller_materials
            .remove(&handle)
            .ok_or(PhysicsError::ControllerNotFound)?;
        Ok(())
    }
}

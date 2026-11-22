use std::{
    cell::RefCell,
    rc::{Rc, Weak},
    sync::atomic::{AtomicBool, Ordering},
};

#[cfg(any(test, feature = "memory-tracking"))]
use physx::foundation::TrackingAllocator;
use physx::physics::PhysicsFoundationBuilder;

use crate::{
    PhysicsError,
    native::{NativePhysics, NativeShape},
};

static FOUNDATION_IN_USE: AtomicBool = AtomicBool::new(false);

thread_local! {
    static CONTEXT: RefCell<Weak<RefCell<PhysicsContext>>> = const { RefCell::new(Weak::new()) };
}

pub(crate) struct PhysicsContext {
    pub(crate) physics: NativePhysics,
    // Drop the foundation before allowing another thread to create one.
    _reservation: FoundationReservation,
}

struct FoundationReservation;

impl Drop for FoundationReservation {
    fn drop(&mut self) {
        FOUNDATION_IN_USE.store(false, Ordering::Release);
    }
}

impl PhysicsContext {
    pub(crate) fn acquire() -> Result<Rc<RefCell<Self>>, PhysicsError> {
        CONTEXT.with(|slot| {
            if let Some(context) = slot.borrow().upgrade() {
                return Ok(context);
            }
            FOUNDATION_IN_USE
                .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
                .map_err(|_| PhysicsError::FoundationOnAnotherThread)?;
            let reservation = FoundationReservation;
            #[cfg(not(any(test, feature = "memory-tracking")))]
            let builder = PhysicsFoundationBuilder::default();
            #[cfg(any(test, feature = "memory-tracking"))]
            let builder = PhysicsFoundationBuilder::new(TrackingAllocator::default());
            let physics = builder
                .build::<NativeShape>()
                .ok_or(PhysicsError::InitializationFailed)?;
            let context = Rc::new(RefCell::new(Self {
                physics,
                _reservation: reservation,
            }));
            *slot.borrow_mut() = Rc::downgrade(&context);
            Ok(context)
        })
    }
}

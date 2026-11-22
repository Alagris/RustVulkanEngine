use crate::ecs::component::ComponentInstance;
use crate::ecs::{Component, GameObject, Scene};
use crate::graphics::{
    Graphics, InstanceBuffer, InstanceBufferHandle, InstanceData, Mesh as GraphicsMesh,
};
use anyhow::Result;
use std::any::Any;

#[crate::ecs::component(
    deserialize_with = crate::render_loop::deserialize_instanced_mesh_component
)]
#[derive(crate::ecs::Component)]
pub struct InstancedMesh {
    mesh: GraphicsMesh,
    instances: InstanceBufferHandle,
}

impl InstancedMesh {
    pub fn new<T: InstanceData>(mesh: GraphicsMesh, instances: &InstanceBuffer<T>) -> Self {
        Self {
            instance: ComponentInstance::default(),
            mesh,
            instances: instances.erase(),
        }
    }
}

impl Component for InstancedMesh {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn start(&mut self, _scene: &Scene, _game_object: &GameObject) -> bool {
        true
    }

    fn update(
        &mut self,
        _scene: &Scene,
        _game_object: &GameObject,
        graphics: &mut Graphics,
    ) -> Result<()> {
        graphics.draw_instanced_buffer(&self.mesh, self.instances.clone())
    }
}

use crate::ecs::component::ComponentInstance;
use crate::ecs::{Component, GameObject, Scene};
use crate::graphics::{Graphics, Mesh as GraphicsMesh};
use anyhow::Result;
use std::any::Any;

#[crate::ecs::component(deserialize_with = crate::render_loop::deserialize_mesh_component)]
#[derive(crate::ecs::Component)]
pub struct Mesh {
    mesh: GraphicsMesh,
}

impl Mesh {
    pub fn new(mesh: GraphicsMesh) -> Self {
        Self {
            instance: ComponentInstance::default(),
            mesh,
        }
    }

    pub fn mesh(&self) -> &GraphicsMesh {
        &self.mesh
    }
}

impl Component for Mesh {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn try_start(
        &mut self,
        _scene: &Scene,
        _game_object: &GameObject,
    ) -> Result<crate::ecs::Features> {
        Ok(crate::ecs::Features {
            updatable: true,
            ..Default::default()
        })
    }

    fn update(
        &mut self,
        _scene: &Scene,
        game_object: &GameObject,
        graphics: &mut Graphics,
    ) -> Result<()> {
        graphics.draw(&self.mesh, game_object.world_transform())
    }
}

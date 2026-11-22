use crate::ecs::component::ComponentInstance;
use crate::ecs::{Component, GameObject, Scene};
use crate::graphics::{
    Graphics, InstanceBuffer, InstanceBufferHandle, InstanceData, Mesh as GraphicsMesh,
};
use crate::material::Material;
use anyhow::Result;
use std::any::Any;
use std::sync::Arc;

#[crate::ecs::component]
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

    fn start(&mut self, _scene: &Scene, _game_object: &GameObject) -> bool {
        true
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

#[crate::ecs::component]
#[derive(crate::ecs::Component)]
pub struct ProceduralMesh {
    material: Arc<Material>,
    vertices: i32,
    instances: i32,
}

impl ProceduralMesh {
    pub fn new(material: Arc<Material>, vertices: i32, instances: i32) -> Result<Self> {
        if vertices <= 0 || instances <= 0 {
            anyhow::bail!("procedural mesh vertex and instance counts must be positive");
        }
        Ok(Self {
            instance: ComponentInstance::default(),
            material,
            vertices,
            instances,
        })
    }
}

impl Component for ProceduralMesh {
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
        graphics.draw_procedural(Arc::clone(&self.material), self.vertices, self.instances)
    }
}

#[crate::ecs::component]
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ecs::{ComponentRegistry, Scene};
    use crate::graphics::DrawCommand;
    use crate::material::ShaderPass;
    use crate::render_pipeline::{CullMode, RenderQueue, ShaderInputLayout};

    fn material() -> Arc<Material> {
        Arc::new(
            Material::new(vec![ShaderPass {
                name: "Main".to_owned(),
                vertex_entry: "VSMain".to_owned(),
                fragment_entry: "PSMain".to_owned(),
                vertex_spirv: Arc::from([]),
                fragment_spirv: Arc::from([]),
                cull: CullMode::Back,
                queue: RenderQueue::Opaque,
                vertex_inputs: ShaderInputLayout::default(),
                instance_inputs: ShaderInputLayout::default(),
            }])
            .unwrap(),
        )
    }

    #[test]
    fn procedural_mesh_component_queues_its_draw_call() {
        let registry = ComponentRegistry::new();
        let mut scene = Scene::from_yaml("root:\n  name: Root\n", &registry).unwrap();
        scene.initialize().unwrap();
        let mut graphics = Graphics::default();
        let mut procedural = ProceduralMesh::new(material(), 24, 2).unwrap();
        procedural
            .update(&scene, &scene.root, &mut graphics)
            .unwrap();
        let batches = graphics.optimize();
        assert_eq!(batches.len(), 1);
        assert!(matches!(
            batches[0].commands.as_slice(),
            [DrawCommand::Procedural {
                vertices: 24,
                instances: 2,
                ..
            }]
        ));
    }

    #[test]
    fn rejects_non_positive_procedural_counts() {
        assert!(ProceduralMesh::new(material(), 0, 1).is_err());
        assert!(ProceduralMesh::new(material(), 3, 0).is_err());
    }
}

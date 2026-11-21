use super::buffer::{InstanceBuffer, InstanceBufferHandle, InstanceData};
use super::mesh::{Mesh, validate_material_layouts_for_draw};
use super::vertex::Transform;
use crate::material::Material;
use crate::render_pipeline::RenderQueue;
use anyhow::{Result, bail};
use cgmath::SquareMatrix;
use glam::{Mat4, Vec4};
use std::sync::Arc;

#[derive(Clone)]
pub(crate) enum DrawCommand {
    Mesh {
        mesh: Arc<Mesh>,
        submesh_index: usize,
        transforms: Vec<Mat4>,
    },
    Instanced {
        mesh: Arc<Mesh>,
        submesh_index: usize,
        instances: InstanceBufferHandle,
    },
    Procedural {
        material: Arc<Material>,
        vertices: u32,
        instances: u32,
    },
}

impl DrawCommand {
    fn sort_key(&self) -> (u64, u64, usize) {
        match self {
            Self::Mesh {
                mesh,
                submesh_index,
                ..
            }
            | Self::Instanced {
                mesh,
                submesh_index,
                ..
            } => (
                mesh.materials[mesh.submeshes[*submesh_index].material_index].id(),
                mesh.id(),
                *submesh_index,
            ),
            Self::Procedural {
                material, vertices, ..
            } => (material.id(), u64::MAX, *vertices as usize),
        }
    }
    fn is_transparent(&self) -> bool {
        let material = match self {
            Self::Mesh {
                mesh,
                submesh_index,
                ..
            }
            | Self::Instanced {
                mesh,
                submesh_index,
                ..
            } => &mesh.materials[mesh.submeshes[*submesh_index].material_index],
            Self::Procedural { material, .. } => material,
        };
        material
            .passes
            .iter()
            .any(|pass| pass.queue == RenderQueue::Transparent)
    }
}

#[derive(Clone)]
pub(crate) struct DrawBatch {
    pub(crate) commands: Vec<DrawCommand>,
}

#[derive(Default)]
pub struct Graphics {
    queued: Vec<DrawCommand>,
}

impl Graphics {
    pub fn draw(&mut self, mesh: &Mesh, transform: Transform) -> Result<()> {
        assert!(
            transform_is_finite(transform) && transform.determinant().abs() > f32::EPSILON,
            "draw transforms must be finite and invertible"
        );
        assert!(
            !mesh.submeshes.is_empty(),
            "draw mesh must contain submeshes"
        );
        validate_material_layouts_for_draw(mesh, &<Mat4 as InstanceData>::layout())?;
        let transform = to_glam_transform(transform);
        let mesh = Arc::new(mesh.clone());
        for submesh_index in 0..mesh.submeshes.len() {
            self.queued.push(DrawCommand::Mesh {
                mesh: Arc::clone(&mesh),
                submesh_index,
                transforms: vec![transform],
            });
        }

        fn transform_is_finite(transform: Transform) -> bool {
            [
                transform.x.x,
                transform.x.y,
                transform.x.z,
                transform.x.w,
                transform.y.x,
                transform.y.y,
                transform.y.z,
                transform.y.w,
                transform.z.x,
                transform.z.y,
                transform.z.z,
                transform.z.w,
                transform.w.x,
                transform.w.y,
                transform.w.z,
                transform.w.w,
            ]
            .iter()
            .all(|value| value.is_finite())
        }

        fn to_glam_transform(transform: Transform) -> Mat4 {
            Mat4::from_cols(
                Vec4::new(transform.x.x, transform.x.y, transform.x.z, transform.x.w),
                Vec4::new(transform.y.x, transform.y.y, transform.y.z, transform.y.w),
                Vec4::new(transform.z.x, transform.z.y, transform.z.z, transform.z.w),
                Vec4::new(transform.w.x, transform.w.y, transform.w.z, transform.w.w),
            )
        }
        Ok(())
    }
    pub fn draw_procedural(
        &mut self,
        material: Arc<Material>,
        vertices: i32,
        instances: i32,
    ) -> Result<()> {
        if vertices <= 0 || instances <= 0 {
            bail!("procedural vertex and instance counts must be positive");
        }
        self.queued.push(DrawCommand::Procedural {
            material,
            vertices: vertices as u32,
            instances: instances as u32,
        });
        Ok(())
    }
    pub fn draw_instanced<T: InstanceData>(
        &mut self,
        mesh: &Mesh,
        instances: &InstanceBuffer<T>,
    ) -> Result<()> {
        self.draw_instanced_buffer(mesh, instances.erase())
    }

    pub(crate) fn draw_instanced_buffer(
        &mut self,
        mesh: &Mesh,
        instances: InstanceBufferHandle,
    ) -> Result<()> {
        assert!(
            !mesh.submeshes.is_empty(),
            "instanced draw mesh must contain submeshes"
        );
        assert!(instances.len() > 0, "instanced draw must contain instances");
        if instances.len() == 0 {
            bail!("instanced draw must contain at least one instance");
        }
        validate_material_layouts_for_draw(mesh, instances.layout())?;
        for material in &mesh.materials {
            Mesh::validate_material_layouts(material, &mesh.vertices.layout, instances.layout())?;
        }
        let mesh = Arc::new(mesh.clone());
        for submesh_index in 0..mesh.submeshes.len() {
            self.queued.push(DrawCommand::Instanced {
                mesh: Arc::clone(&mesh),
                submesh_index,
                instances: instances.clone(),
            });
        }
        Ok(())
    }
    pub(crate) fn optimize(&mut self) -> Vec<DrawBatch> {
        let mut opaque = Vec::new();
        let mut transparent = Vec::new();
        for command in self.queued.drain(..) {
            if command.is_transparent() {
                transparent.push(command);
            } else {
                opaque.push(command);
            }
        }
        opaque.sort_by_key(DrawCommand::sort_key);
        let mut batches: Vec<DrawBatch> = Vec::new();
        for command in opaque.into_iter().chain(transparent) {
            let mergeable = matches!(command, DrawCommand::Mesh { .. });
            let merge = mergeable
                && batches.last().is_some_and(|batch| {
                    matches!(batch.commands.first(), Some(DrawCommand::Mesh { .. }))
                        && batch.commands[0].sort_key() == command.sort_key()
                });
            if merge {
                if let DrawCommand::Mesh { transforms, .. } = command {
                    assert!(
                        !batches.is_empty(),
                        "mergeable batch must have a preceding batch"
                    );
                    let batch = match batches.last_mut() {
                        Some(batch) => batch,
                        None => unreachable!("preceding mergeable batch was asserted"),
                    };
                    assert_eq!(
                        batch.commands.len(),
                        1,
                        "mergeable mesh batch must contain exactly one source command"
                    );
                    if let DrawCommand::Mesh {
                        transforms: previous,
                        ..
                    } = &mut batch.commands[0]
                    {
                        previous.extend(transforms);
                    }
                }
            } else {
                batches.push(DrawBatch {
                    commands: vec![command],
                });
            }
        }
        batches
    }
}

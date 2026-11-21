use crate::graphics::{GpuContext, Material, Mesh, Submesh, Vertex};
use anyhow::Result;
use glam::{Vec3A, Vec4};
use std::sync::Arc;

pub fn cube(context: &GpuContext, material: Arc<Material>) -> Result<Mesh> {
    let faces = [
        (
            Vec4::new(1.0, 0.3, 0.3, 1.0),
            [
                [-0.5, -0.5, 0.5],
                [0.5, -0.5, 0.5],
                [0.5, 0.5, 0.5],
                [-0.5, 0.5, 0.5],
            ],
        ),
        (
            Vec4::new(0.3, 1.0, 0.3, 1.0),
            [
                [0.5, -0.5, -0.5],
                [-0.5, -0.5, -0.5],
                [-0.5, 0.5, -0.5],
                [0.5, 0.5, -0.5],
            ],
        ),
        (
            Vec4::new(0.3, 0.3, 1.0, 1.0),
            [
                [-0.5, -0.5, -0.5],
                [-0.5, -0.5, 0.5],
                [-0.5, 0.5, 0.5],
                [-0.5, 0.5, -0.5],
            ],
        ),
        (
            Vec4::new(1.0, 1.0, 0.3, 1.0),
            [
                [0.5, -0.5, 0.5],
                [0.5, -0.5, -0.5],
                [0.5, 0.5, -0.5],
                [0.5, 0.5, 0.5],
            ],
        ),
        (
            Vec4::new(0.3, 1.0, 1.0, 1.0),
            [
                [-0.5, 0.5, 0.5],
                [0.5, 0.5, 0.5],
                [0.5, 0.5, -0.5],
                [-0.5, 0.5, -0.5],
            ],
        ),
        (
            Vec4::new(1.0, 0.3, 1.0, 1.0),
            [
                [-0.5, -0.5, -0.5],
                [0.5, -0.5, -0.5],
                [0.5, -0.5, 0.5],
                [-0.5, -0.5, 0.5],
            ],
        ),
    ];
    let mut vertices = Vec::with_capacity(24);
    let mut indices = Vec::with_capacity(36);
    for (color, corners) in faces {
        let first = vertices.len() as u32;
        vertices.extend(corners.into_iter().map(|position| Vertex {
            position: Vec3A::from_array(position),
            color,
        }));
        indices.extend_from_slice(&[first, first + 1, first + 2, first, first + 2, first + 3]);
    }

    Mesh::new(
        context,
        &vertices,
        &indices,
        vec![material],
        vec![Submesh {
            first_index: 0,
            index_count: 36,
            material_index: 0,
        }],
    )
}

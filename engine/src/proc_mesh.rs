use crate::graphics::{GpuContext, Mesh, Submesh, Vertex};
use crate::material::Material;
use anyhow::{Result, bail};
use glam::{Vec3A, Vec4};
use std::sync::Arc;

pub fn cube(context: &GpuContext, material: Arc<Material>) -> Result<Mesh> {
    cube_with_size(context, material, Vec3A::ONE)
}

pub fn cube_with_size(context: &GpuContext, material: Arc<Material>, size: Vec3A) -> Result<Mesh> {
    assert!(Arc::strong_count(&material) >= 1);
    if !size.is_finite() || size.min_element() <= 0.0 {
        bail!("cube dimensions must be finite and greater than zero");
    }
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
            position: Vec3A::new(
                position[0] * size.x,
                position[1] * size.y,
                position[2] * size.z,
            ),
            color,
        }));
        indices.extend_from_slice(&[first, first + 1, first + 2, first, first + 2, first + 3]);
    }

    assert_eq!(vertices.len(), 24, "cube has four vertices per face");
    assert_eq!(indices.len(), 36, "cube has two triangles per face");
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

pub fn plane(context: &GpuContext, material: Arc<Material>) -> Result<Mesh> {
    plane_with_segments(context, material, 10.0, 10.0, 1, 1)
}

pub fn plane_with_size(
    context: &GpuContext,
    material: Arc<Material>,
    width: f32,
    depth: f32,
) -> Result<Mesh> {
    plane_with_segments(context, material, width, depth, 1, 1)
}

pub fn plane_with_segments(
    context: &GpuContext,
    material: Arc<Material>,
    width: f32,
    depth: f32,
    segments_x: u32,
    segments_z: u32,
) -> Result<Mesh> {
    if !width.is_finite() || !depth.is_finite() || width <= 0.0 || depth <= 0.0 {
        bail!("plane dimensions must be finite and greater than zero");
    }
    if segments_x == 0 || segments_z == 0 {
        bail!("plane segment counts must be greater than zero");
    }
    let columns = segments_x as usize + 1;
    let rows = segments_z as usize + 1;
    let vertex_count = columns
        .checked_mul(rows)
        .filter(|&count| count <= u32::MAX as usize)
        .ok_or_else(|| anyhow::anyhow!("plane vertex count exceeds the supported index range"))?;
    let index_count = segments_x
        .checked_mul(segments_z)
        .and_then(|count| count.checked_mul(6))
        .ok_or_else(|| anyhow::anyhow!("plane index count overflow"))?
        as usize;

    let color = Vec4::new(0.65, 0.68, 0.72, 1.0);
    let mut vertices = Vec::with_capacity(vertex_count);
    for z in 0..=segments_z {
        for x in 0..=segments_x {
            let x_fraction = x as f32 / segments_x as f32;
            let z_fraction = z as f32 / segments_z as f32;
            vertices.push(Vertex {
                position: Vec3A::new((x_fraction - 0.5) * width, 0.0, (z_fraction - 0.5) * depth),
                color,
            });
        }
    }
    let mut indices = Vec::with_capacity(index_count);
    for z in 0..segments_z {
        for x in 0..segments_x {
            let top_left = (z as usize * columns + x as usize) as u32;
            let bottom_left = top_left + columns as u32;
            let bottom_right = bottom_left + 1;
            let top_right = top_left + 1;
            indices.extend_from_slice(&[
                top_left,
                bottom_left,
                bottom_right,
                top_left,
                bottom_right,
                top_right,
            ]);
        }
    }
    assert_eq!(vertices.len(), vertex_count);
    assert_eq!(indices.len(), index_count);
    Mesh::new(
        context,
        &vertices,
        &indices,
        vec![material],
        vec![Submesh {
            first_index: 0,
            index_count: indices.len() as u32,
            material_index: 0,
        }],
    )
}

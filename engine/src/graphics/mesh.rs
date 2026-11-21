use super::buffer::ComputeBuffer;
use super::buffer::{IndexBuffer, InstanceData, VertexBuffer};
use super::gpu_context::GpuContext;
use super::vertex::Vertex;
use super::vertex_layout::VertexLayout;
use crate::material::Material;
use anyhow::{Result, anyhow, bail};
use glam::Mat4;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use vulkanalia::prelude::v1_0::*;

static NEXT_MESH_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Submesh {
    pub first_index: u32,
    pub index_count: u32,
    pub material_index: usize,
}

#[derive(Clone)]
pub struct Mesh {
    id: u64,
    pub vertices: VertexBuffer,
    pub indices: IndexBuffer,
    pub materials: Vec<Arc<Material>>,
    pub submeshes: Vec<Submesh>,
}

impl Mesh {
    pub fn new(
        context: &GpuContext,
        vertices: &[Vertex],
        indices: &[u32],
        materials: Vec<Arc<Material>>,
        submeshes: Vec<Submesh>,
    ) -> Result<Self> {
        assert!(context.device().handle() != vk::Device::null());
        if vertices.is_empty() || indices.is_empty() || materials.is_empty() || submeshes.is_empty()
        {
            bail!("meshes require vertices, indices, materials, and submeshes");
        }
        if indices
            .iter()
            .any(|&index| index as usize >= vertices.len())
        {
            bail!("mesh index is outside the vertex array");
        }
        let vertex_buffer = VertexBuffer::new(context, vertices)?;
        let index_buffer = IndexBuffer::new(context, indices)?;
        Self::from_buffers(
            vertex_buffer,
            index_buffer,
            materials,
            submeshes,
            <Mat4 as InstanceData>::layout(),
        )
    }

    pub fn from_buffers(
        vertices: VertexBuffer,
        indices: IndexBuffer,
        materials: Vec<Arc<Material>>,
        submeshes: Vec<Submesh>,
        instance_layout: VertexLayout,
    ) -> Result<Self> {
        if vertices.vertex_count() == 0
            || indices.len() == 0
            || materials.is_empty()
            || submeshes.is_empty()
        {
            bail!("meshes require vertices, indices, materials, and submeshes");
        }
        for material in &materials {
            Self::validate_material_layouts(material, &vertices.layout, &instance_layout)?;
        }
        for submesh in &submeshes {
            let end = submesh
                .first_index
                .checked_add(submesh.index_count)
                .ok_or_else(|| anyhow!("submesh index range overflow"))?;
            if submesh.index_count == 0
                || end as usize > indices.len()
                || submesh.material_index >= materials.len()
            {
                bail!("submesh has an invalid index range or material index");
            }
        }
        Ok(Self {
            id: NEXT_MESH_ID.fetch_add(1, Ordering::Relaxed),
            vertices,
            indices,
            materials,
            submeshes,
        })
    }

    pub(crate) fn validate_material_layouts(
        material: &Material,
        vertex_layout: &VertexLayout,
        instance_layout: &VertexLayout,
    ) -> Result<()> {
        let expected_vertex_stride = material
            .passes
            .iter()
            .map(|pass| pass.vertex_inputs.stride)
            .max()
            .unwrap_or(0);
        let expected_instance_stride = material
            .passes
            .iter()
            .map(|pass| pass.instance_inputs.stride)
            .max()
            .unwrap_or(0);
        if expected_vertex_stride != vertex_layout.stride {
            bail!(
                "mesh vertex stride {} does not match material '{}' stride {}",
                vertex_layout.stride,
                material.id(),
                expected_vertex_stride
            );
        }
        if expected_instance_stride != instance_layout.stride {
            bail!(
                "mesh instance stride {} does not match material '{}' stride {}",
                instance_layout.stride,
                material.id(),
                expected_instance_stride
            );
        }
        for pass in &material.passes {
            vertex_layout.validate_shader_inputs(&pass.vertex_inputs)?;
            instance_layout.validate_shader_inputs(&pass.instance_inputs)?;
        }
        Ok(())
    }

    pub fn id(&self) -> u64 {
        self.id
    }
}

pub(crate) fn validate_material_layouts_for_draw(
    mesh: &Mesh,
    instance_layout: &VertexLayout,
) -> Result<()> {
    for material in &mesh.materials {
        Mesh::validate_material_layouts(material, &mesh.vertices.layout, instance_layout)?;
    }
    Ok(())
}

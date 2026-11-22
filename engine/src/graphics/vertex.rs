use super::vertex_layout::VertexLayout;
use crate::render_pipeline::{ShaderInputAttribute, ShaderInputFormat};
use bytemuck::{Pod, Zeroable};
use glam::{Vec3A, Vec4};

#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
#[repr(C)]
pub struct Vertex {
    pub position: Vec3A,
    pub color: Vec4,
}

impl Vertex {
    pub fn layout() -> VertexLayout {
        VertexLayout {
            stride: std::mem::size_of::<Self>() as u32,
            attributes: vec![
                ShaderInputAttribute {
                    location: 0,
                    format: ShaderInputFormat::Float32x3,
                    offset: std::mem::offset_of!(Self, position) as u32,
                },
                ShaderInputAttribute {
                    location: 1,
                    format: ShaderInputFormat::Float32x4,
                    offset: std::mem::offset_of!(Self, color) as u32,
                },
            ],
        }
    }
}

pub type Transform = cgmath::Matrix4<f32>;

#[derive(Clone, Copy, Debug, Pod, Zeroable)]
#[repr(C)]
pub struct FrameUniform {
    pub view_projection: [[f32; 4]; 4],
    pub view: [[f32; 4]; 4],
    pub shadow_view_projection: [[[f32; 4]; 4]; 4],
    pub cascade_splits: [f32; 4],
    pub cascade_index: u32,
    pub padding: [u32; 3],
    pub directional_light_direction: [f32; 4],
    pub directional_light_color_intensity: [f32; 4],
    pub camera_position: [f32; 4],
}

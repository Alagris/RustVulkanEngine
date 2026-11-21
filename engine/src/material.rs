use crate::render_pipeline::{CullMode, RenderQueue, ShaderInputLayout};
use anyhow::{Result, bail};
use glam::{Mat4, Vec2, Vec4};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_MATERIAL_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum UniformValue {
    Float(f32),
    Int(i32),
    Vec2(Vec2),
    Vec3(glam::Vec3),
    Vec4(Vec4),
    Mat4(Mat4),
}

#[derive(Clone, Debug)]
pub struct ShaderPass {
    pub name: String,
    pub vertex_entry: String,
    pub fragment_entry: String,
    pub vertex_spirv: Arc<[u32]>,
    pub fragment_spirv: Arc<[u32]>,
    pub cull: CullMode,
    pub queue: RenderQueue,
    pub vertex_inputs: ShaderInputLayout,
    pub instance_inputs: ShaderInputLayout,
}

#[derive(Clone, Debug)]
pub struct Material {
    id: u64,
    pub passes: Vec<ShaderPass>,
    uniforms: HashMap<String, UniformValue>,
}

impl Material {
    pub fn new(passes: Vec<ShaderPass>) -> Result<Self> {
        assert!(
            !passes.is_empty(),
            "Material::new requires at least one shader pass"
        );
        if passes.is_empty() {
            bail!("materials require at least one compiled shader pass");
        }
        for pass in &passes {
            assert!(
                !pass.name.trim().is_empty(),
                "shader pass names must not be empty"
            );
            assert!(
                !pass.vertex_entry.trim().is_empty() && !pass.fragment_entry.trim().is_empty(),
                "shader pass entry points must not be empty"
            );
        }
        Ok(Self {
            id: NEXT_MATERIAL_ID.fetch_add(1, Ordering::Relaxed),
            passes,
            uniforms: HashMap::new(),
        })
    }

    pub fn id(&self) -> u64 {
        assert!(self.id != 0, "material IDs must be allocated and non-zero");
        self.id
    }

    pub fn set_uniform(&mut self, name: impl Into<String>, value: UniformValue) -> Result<()> {
        let name = name.into();
        assert!(
            name.len() <= 1024,
            "uniform names must remain within the engine name limit"
        );
        if name.trim().is_empty() {
            bail!("uniform name must not be empty");
        }
        self.uniforms.insert(name, value);
        Ok(())
    }

    pub fn uniform(&self, name: &str) -> Option<&UniformValue> {
        assert!(!name.is_empty(), "uniform lookup names must not be empty");
        self.uniforms.get(name)
    }

    pub fn pass(&self, name: &str) -> Option<&ShaderPass> {
        assert!(
            !name.is_empty(),
            "shader pass lookup names must not be empty"
        );
        self.passes.iter().find(|pass| pass.name == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn material_uniforms_are_readable_by_name() {
        let mut material = Material::new(vec![ShaderPass {
            name: "Forward".to_owned(),
            vertex_entry: "VSMain".to_owned(),
            fragment_entry: "PSMain".to_owned(),
            vertex_spirv: Arc::from([]),
            fragment_spirv: Arc::from([]),
            cull: CullMode::Back,
            queue: RenderQueue::Opaque,
            vertex_inputs: ShaderInputLayout::default(),
            instance_inputs: ShaderInputLayout::default(),
        }])
        .unwrap();
        material
            .set_uniform("roughness", UniformValue::Float(0.5))
            .unwrap();
        assert_eq!(
            material.uniform("roughness"),
            Some(&UniformValue::Float(0.5))
        );
        assert!(material.pass("Forward").is_some());
    }
}

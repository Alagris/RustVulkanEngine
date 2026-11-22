use serde::Deserialize;

#[derive(Deserialize)]
#[serde(default)]
pub(crate) struct PrimitiveMeshSettings {
    pub(crate) primitive: String,
    pub(crate) material: String,
    pub(crate) size: [f32; 3],
    pub(crate) width: f32,
    pub(crate) depth: f32,
    pub(crate) segments_x: u32,
    pub(crate) segments_z: u32,
}

impl Default for PrimitiveMeshSettings {
    fn default() -> Self {
        Self {
            primitive: "cube".to_owned(),
            material: String::new(),
            size: [1.0; 3],
            width: 10.0,
            depth: 10.0,
            segments_x: 1,
            segments_z: 1,
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct ProceduralMeshSettings {
    pub(crate) material: String,
    pub(crate) vertices: i32,
    #[serde(default = "one")]
    pub(crate) instances: i32,
}

#[derive(Deserialize)]
pub(crate) struct InstancedMeshSettings {
    #[serde(flatten)]
    pub(crate) primitive: PrimitiveMeshSettings,
    pub(crate) transforms: Vec<[f32; 16]>,
}

#[derive(Deserialize)]
pub(crate) struct SkinnedMeshSettings {
    pub(crate) fbx: String,
    pub(crate) material: String,
    pub(crate) mesh: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct AnimatorSettings {
    pub(crate) fbx: String,
}

fn one() -> i32 {
    1
}

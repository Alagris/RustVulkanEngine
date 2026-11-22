use crate::ecs::component::ComponentInstance;
use crate::ecs::{Component, GameObject, Scene};
use crate::graphics::{
    GpuContext, GpuSkinner, Graphics, IndexBuffer, Mesh as GraphicsMesh, Submesh, Vertex,
};
use crate::material::Material;
use anyhow::{Context, Result, anyhow, bail};
use glam::{Mat4, Vec3};
use std::any::Any;
use std::sync::Arc;

pub struct Bone {
    pub name: String,
    pub parent: Option<usize>,
    pub hierarchy_node: usize,
    pub inverse_bind_matrix: Mat4,
    pub pose_matrix: Mat4,
}

#[crate::ecs::component(
    deserialize_with = crate::render_loop::deserialize_skinned_mesh_component
)]
#[derive(crate::ecs::Component)]
pub struct SkinnedMesh {
    pub name: String,
    pub mesh: GraphicsMesh,
    pub bones: Vec<Bone>,
    skin: Option<fbx_reader::Skin>,
    control_points: Vec<usize>,
    geometry_to_world: fbx_reader::transform::Mat4,
    unit_meters: f64,
    gpu_skinner: Option<GpuSkinner>,
}

impl SkinnedMesh {
    pub fn new(name: String, mesh: GraphicsMesh, bones: Vec<Bone>) -> Self {
        Self {
            instance: ComponentInstance::default(),
            name,
            mesh,
            bones,
            skin: None,
            control_points: Vec::new(),
            geometry_to_world: fbx_reader::transform::IDENTITY,
            unit_meters: 1.0,
            gpu_skinner: None,
        }
    }

    pub(crate) fn from_fbx(
        context: &GpuContext,
        name: String,
        vertices: Vec<Vertex>,
        indices: Vec<u32>,
        material: Arc<Material>,
        bones: Vec<Bone>,
        skin: fbx_reader::Skin,
        control_points: Vec<usize>,
        geometry_to_world: fbx_reader::transform::Mat4,
        unit_meters: f64,
    ) -> Result<Self> {
        if vertices.len() != control_points.len() || vertices.is_empty() || indices.is_empty() {
            bail!("FBX skin vertex and control-point data is inconsistent");
        }
        let transforms = vertex_transforms(
            &skin,
            &control_points,
            geometry_to_world,
            unit_meters,
            &bones,
        )?;
        let (vertex_buffer, gpu_skinner) = GpuSkinner::new(context, &vertices, &transforms)
            .with_context(|| format!("failed to initialize GPU skinning for '{name}'"))?;
        let index_count = u32::try_from(indices.len())
            .context("FBX mesh has more than u32::MAX triangle indices")?;
        let mesh = GraphicsMesh::from_buffers(
            vertex_buffer,
            IndexBuffer::new(context, &indices)?,
            vec![material],
            vec![Submesh {
                first_index: 0,
                index_count,
                material_index: 0,
            }],
            <glam::Mat4 as crate::graphics::InstanceData>::layout(),
        )?;
        Ok(Self {
            instance: ComponentInstance::default(),
            name,
            mesh,
            bones,
            skin: Some(skin),
            control_points,
            geometry_to_world,
            unit_meters,
            gpu_skinner: Some(gpu_skinner),
        })
    }

    pub(crate) fn update_skinning(&mut self) -> Result<()> {
        let skin = self
            .skin
            .as_ref()
            .ok_or_else(|| anyhow!("mesh '{}' has no skin data", self.name))?;
        let transforms = vertex_transforms(
            skin,
            &self.control_points,
            self.geometry_to_world,
            self.unit_meters,
            &self.bones,
        )?;
        self.gpu_skinner
            .as_mut()
            .ok_or_else(|| anyhow!("mesh '{}' has no GPU skinning pipeline", self.name))?
            .update_transforms(&transforms)
    }

    pub(crate) fn is_skinned(&self) -> bool {
        self.skin.is_some()
    }
}

fn vertex_transforms(
    skin: &fbx_reader::Skin,
    control_points: &[usize],
    geometry_to_world: fbx_reader::transform::Mat4,
    unit_meters: f64,
    bones: &[Bone],
) -> Result<Vec<Mat4>> {
    if !unit_meters.is_finite() || unit_meters <= 0.0 {
        bail!("FBX unit scale must be finite and positive");
    }
    let cluster_matrices = bones
        .iter()
        .map(|bone| crate::ecs::animator::glam_matrix_to_fbx(bone.pose_matrix))
        .collect::<Vec<_>>();
    if cluster_matrices.len() != skin.clusters.len() {
        bail!("FBX skin clusters do not match the mesh bone palette");
    }
    let unit_scale = Mat4::from_scale(Vec3::splat(unit_meters as f32));
    control_points
        .iter()
        .map(|&control_point| {
            if control_point >= skin.points() {
                bail!("FBX mesh references a control point outside its skin");
            }
            let matrix = skin
                .point_matrix(control_point, &cluster_matrices)
                .unwrap_or(geometry_to_world);
            let transform = unit_scale * crate::ecs::animator::fbx_matrix_to_glam(matrix);
            if !transform.is_finite() {
                bail!("FBX skin produced a non-finite vertex transform");
            }
            Ok(transform)
        })
        .collect()
}

impl Component for SkinnedMesh {
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
        Ok(crate::ecs::UPDATE)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anim::AnimationClip;

    #[test]
    fn per_vertex_skin_transforms_follow_the_current_bone_pose() {
        let bytes = fbx_reader::write::to_bytes(7400, &fbx_reader::write::skinned_scene(), true);
        let limits = fbx_reader::Limits::default();
        let source = fbx_reader::Scene::parse(&bytes, &limits).unwrap();
        let instance = source.mesh_instances().into_iter().next().unwrap();
        let skin = source.skin(instance.geometry_id, &limits).unwrap().unwrap();
        let mut bones = skin
            .clusters
            .iter()
            .map(|cluster| {
                let node = &source.hierarchy.nodes[cluster.bone];
                Bone {
                    name: node.name.clone(),
                    parent: node.parent,
                    hierarchy_node: cluster.bone,
                    inverse_bind_matrix: crate::ecs::animator::fbx_matrix_to_glam(
                        cluster.geometry_to_bone,
                    ),
                    pose_matrix: crate::ecs::animator::fbx_matrix_to_glam(cluster.bind_to_world)
                        * crate::ecs::animator::fbx_matrix_to_glam(cluster.geometry_to_bone),
                }
            })
            .collect::<Vec<_>>();
        let control_points = (0..source
            .geometry(instance.geometry_id, &limits)
            .unwrap()
            .positions
            .len())
            .collect::<Vec<_>>();
        let bind = vertex_transforms(
            &skin,
            &control_points,
            instance.geometry_to_world,
            source.unit_meters,
            &bones,
        )
        .unwrap();
        let clip = AnimationClip::from_fbx(
            0,
            Arc::new(source.hierarchy.clone()),
            Arc::new(source.animation.clone()),
        )
        .unwrap();
        clip.pose_at(1.0, &mut bones).unwrap();
        let animated = vertex_transforms(
            &skin,
            &control_points,
            instance.geometry_to_world,
            source.unit_meters,
            &bones,
        )
        .unwrap();
        assert!(
            bind.iter()
                .zip(animated)
                .any(|(before, after)| before != &after)
        );
    }
}

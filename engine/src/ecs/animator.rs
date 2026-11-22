use crate::anim::AnimationClip;
use crate::ecs::component::{ComponentInstance, ComponentWeak};
use crate::ecs::{Component, GameObject, Scene, SkinnedMesh};
use crate::graphics::Graphics;
use anyhow::{Result, anyhow, bail};
use glam::Mat4;
use std::any::Any;

#[crate::ecs::component(
    deserialize_with = crate::render_loop::deserialize_animator_component
)]
#[derive(crate::ecs::Component)]
#[require(SkinnedMesh)]
pub struct Animator {
    skinned_mesh: Option<ComponentWeak<SkinnedMesh>>,
    clips: Vec<AnimationClip>,
    active_clip: Option<usize>,
    elapsed: f32,
    playing: bool,
}

impl Animator {
    pub fn new(clips: Vec<AnimationClip>) -> Result<Self> {
        if clips.is_empty() {
            bail!("an Animator requires at least one animation clip");
        }
        Ok(Self {
            skinned_mesh: None,
            instance: ComponentInstance::default(),
            clips,
            active_clip: None,
            elapsed: 0.0,
            playing: false,
        })
    }

    pub fn play(&mut self, clip_name: &str) -> Result<()> {
        let index = self
            .clips
            .iter()
            .position(|clip| clip.name == clip_name)
            .ok_or_else(|| anyhow!("animation clip '{}' was not loaded", clip_name))?;
        self.active_clip = Some(index);
        self.elapsed = 0.0;
        self.playing = true;
        Ok(())
    }

    pub fn stop(&mut self) {
        self.playing = false;
        self.elapsed = 0.0;
    }
}

impl Component for Animator {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn try_start(
        &mut self,
        _scene: &Scene,
        game_object: &GameObject,
    ) -> Result<crate::ecs::Features> {
        self.skinned_mesh = game_object.get_component_weak::<SkinnedMesh>();
        Ok(crate::ecs::UPDATE)
    }

    fn update(
        &mut self,
        scene: &Scene,
        game_object: &GameObject,
        _graphics: &mut Graphics,
    ) -> Result<()> {
        if !self.playing {
            return Ok(());
        }
        if self
            .skinned_mesh
            .as_ref()
            .map_or(true, |skinned_mesh| !skinned_mesh.is_alive())
        {
            self.skinned_mesh = game_object.get_component_weak::<SkinnedMesh>();
        }
        let clip_index = self
            .active_clip
            .ok_or_else(|| anyhow!("playing Animator has no active clip"))?;
        self.elapsed += scene.delta_time();
        if !self.elapsed.is_finite() {
            bail!("Animator elapsed time exceeded its finite range");
        }
        let clip = self
            .clips
            .get(clip_index)
            .ok_or_else(|| anyhow!("Animator active clip index is invalid"))?;
        let result = self
            .skinned_mesh
            .as_ref()
            .and_then(|skinned_mesh| {
                skinned_mesh.with_mut(|skinned_mesh| -> Result<()> {
                    if !skinned_mesh.is_skinned() {
                        bail!("Animator requires a SkinnedMesh loaded from FBX skin data");
                    }
                    clip.pose_at(self.elapsed, &mut skinned_mesh.bones)?;
                    skinned_mesh.update_skinning()
                })
            })
            .ok_or_else(|| anyhow!("Animator requires a SkinnedMesh on the same GameObject"))?;
        result
    }
}

pub(crate) fn glam_matrix_to_fbx(matrix: Mat4) -> fbx_reader::transform::Mat4 {
    let columns = matrix.to_cols_array();
    [
        [
            f64::from(columns[0]),
            f64::from(columns[4]),
            f64::from(columns[8]),
            f64::from(columns[12]),
        ],
        [
            f64::from(columns[1]),
            f64::from(columns[5]),
            f64::from(columns[9]),
            f64::from(columns[13]),
        ],
        [
            f64::from(columns[2]),
            f64::from(columns[6]),
            f64::from(columns[10]),
            f64::from(columns[14]),
        ],
        [
            f64::from(columns[3]),
            f64::from(columns[7]),
            f64::from(columns[11]),
            f64::from(columns[15]),
        ],
    ]
}

pub(crate) fn fbx_matrix_to_glam(matrix: fbx_reader::transform::Mat4) -> Mat4 {
    Mat4::from_cols_array(&[
        matrix[0][0] as f32,
        matrix[1][0] as f32,
        matrix[2][0] as f32,
        matrix[3][0] as f32,
        matrix[0][1] as f32,
        matrix[1][1] as f32,
        matrix[2][1] as f32,
        matrix[3][1] as f32,
        matrix[0][2] as f32,
        matrix[1][2] as f32,
        matrix[2][2] as f32,
        matrix[3][2] as f32,
        matrix[0][3] as f32,
        matrix[1][3] as f32,
        matrix[2][3] as f32,
        matrix[3][3] as f32,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ecs::Bone;
    use std::sync::Arc;

    #[test]
    fn animation_clip_evaluates_fbx_bone_poses() {
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
                    inverse_bind_matrix: fbx_matrix_to_glam(cluster.geometry_to_bone),
                    pose_matrix: fbx_matrix_to_glam(cluster.bind_to_world)
                        * fbx_matrix_to_glam(cluster.geometry_to_bone),
                }
            })
            .collect::<Vec<_>>();
        let initial_poses = bones
            .iter()
            .map(|bone| bone.pose_matrix)
            .collect::<Vec<_>>();
        let clip = AnimationClip::from_fbx(
            0,
            Arc::new(source.hierarchy.clone()),
            Arc::new(source.animation.clone()),
        )
        .unwrap();
        clip.pose_at(1.0, &mut bones).unwrap();
        assert!(
            bones
                .iter()
                .zip(initial_poses)
                .any(|(bone, initial)| bone.pose_matrix != initial)
        );
    }

    #[test]
    fn rejects_empty_animator_clip_lists() {
        assert!(Animator::new(Vec::new()).is_err());
    }
}

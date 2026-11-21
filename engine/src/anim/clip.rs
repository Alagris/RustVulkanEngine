use crate::ecs::animator::fbx_matrix_to_glam;
use crate::ecs::skinned_mesh::Bone;
use anyhow::{Result, anyhow, bail};
use std::sync::Arc;

#[derive(Clone)]
pub struct AnimationClip {
    pub name: String,
    start: f64,
    duration: f64,
    take_index: usize,
    hierarchy: Arc<fbx_reader::Hierarchy>,
    animation: Arc<fbx_reader::Animation>,
}

impl AnimationClip {
    pub(crate) fn from_fbx(
        take_index: usize,
        hierarchy: Arc<fbx_reader::Hierarchy>,
        animation: Arc<fbx_reader::Animation>,
    ) -> Result<Self> {
        let take = animation
            .takes
            .get(take_index)
            .ok_or_else(|| anyhow!("FBX animation take index is invalid"))?;
        if !take.start.is_finite() || !take.duration().is_finite() {
            bail!("FBX animation take has invalid timing values");
        }
        Ok(Self {
            name: take.name.clone(),
            start: take.start,
            duration: take.duration(),
            take_index,
            hierarchy,
            animation,
        })
    }

    pub(crate) fn pose_at(&self, seconds: f32, bones: &mut [Bone]) -> Result<()> {
        if !seconds.is_finite() || seconds < 0.0 {
            bail!("animation time must be finite and non-negative");
        }
        let time = if self.duration > 0.0 {
            self.start + f64::from(seconds).rem_euclid(self.duration)
        } else {
            self.start
        };
        let worlds = self
            .animation
            .worlds(&self.hierarchy, self.take_index, time);
        for bone in bones {
            let world = worlds
                .get(bone.hierarchy_node)
                .ok_or_else(|| anyhow!("animation is missing bone '{}'", bone.name))?;
            bone.pose_matrix = fbx_matrix_to_glam(world.world) * bone.inverse_bind_matrix;
            if !bone.pose_matrix.is_finite() {
                bail!(
                    "animation produced an invalid pose for bone '{}'",
                    bone.name
                );
            }
        }
        Ok(())
    }
}

use crate::anim::AnimationClip;
use crate::ecs::{Bone, SkinnedMesh, animator::fbx_matrix_to_glam};
use crate::graphics::{GpuContext, Mesh, Submesh, Vertex};
use crate::material::Material;
use anyhow::{Context, Result, anyhow, bail};
use glam::{Mat4, Vec3, Vec3A, Vec4};
use std::fs;
use std::path::Path;
use std::sync::Arc;

pub fn load_meshes_from_fbx(
    context: &GpuContext,
    path: impl AsRef<Path>,
    material: Arc<Material>,
) -> Result<Vec<SkinnedMesh>> {
    let path = path.as_ref();
    let bytes = fs::read(path).with_context(|| format!("failed to read FBX {}", path.display()))?;
    let limits = fbx_reader::Limits::default();
    let scene = fbx_reader::Scene::parse(&bytes, &limits)
        .map_err(|error| anyhow!(error.to_string()))
        .with_context(|| format!("failed to parse FBX {}", path.display()))?;
    let mut meshes = Vec::new();

    for instance in scene.mesh_instances() {
        let geometry = scene
            .geometry(instance.geometry_id, &limits)
            .map_err(|error| anyhow!(error.to_string()))
            .with_context(|| format!("failed to read geometry '{}'", instance.name))?;
        let (mut vertices, indices, control_points) = triangle_data(&geometry, &instance.name)?;
        if indices.is_empty() {
            continue;
        }
        let skin = scene
            .skin(instance.geometry_id, &limits)
            .map_err(|error| anyhow!(error.to_string()))
            .with_context(|| format!("failed to read skin for '{}'", instance.name))?;
        let mut bones = Vec::new();
        if let Some(skin) = &skin {
            for cluster in &skin.clusters {
                let node = scene
                    .hierarchy
                    .nodes
                    .get(cluster.bone)
                    .ok_or_else(|| anyhow!("FBX skin references a missing bone node"))?;
                bones.push(Bone {
                    name: node.name.clone(),
                    parent: node.parent,
                    hierarchy_node: cluster.bone,
                    inverse_bind_matrix: fbx_matrix_to_glam(cluster.geometry_to_bone),
                    pose_matrix: fbx_matrix_to_glam(cluster.bind_to_world)
                        * fbx_matrix_to_glam(cluster.geometry_to_bone),
                });
            }
        }
        let skinned_mesh = match skin {
            Some(skin) => SkinnedMesh::from_fbx(
                context,
                instance.name,
                vertices,
                indices,
                Arc::clone(&material),
                bones,
                skin,
                control_points,
                instance.geometry_to_world,
                scene.unit_meters,
            )?,
            None => {
                let transform = Mat4::from_scale(Vec3::splat(scene.unit_meters as f32))
                    * fbx_matrix_to_glam(instance.geometry_to_world);
                for vertex in &mut vertices {
                    vertex.position = Vec3A::from(
                        transform.transform_point3(Vec3::from_array(vertex.position.to_array())),
                    );
                }
                let mesh = Mesh::new(
                    context,
                    &vertices,
                    &indices,
                    vec![Arc::clone(&material)],
                    vec![Submesh {
                        first_index: 0,
                        index_count: u32::try_from(indices.len())
                            .context("FBX mesh has more than u32::MAX triangle indices")?,
                        material_index: 0,
                    }],
                )
                .with_context(|| format!("failed to upload FBX mesh '{}'", instance.name))?;
                SkinnedMesh::new(instance.name, mesh, bones)
            }
        };
        meshes.push(skinned_mesh);
    }

    if meshes.is_empty() {
        bail!(
            "FBX {} contains no renderable triangle meshes",
            path.display()
        );
    }
    Ok(meshes)
}

fn triangle_data(
    geometry: &fbx_reader::Geometry,
    mesh_name: &str,
) -> Result<(Vec<Vertex>, Vec<u32>, Vec<usize>)> {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    let mut control_points = Vec::new();
    for polygon in &geometry.polygons {
        for triangle in geometry.triangulate(polygon) {
            for corner in triangle {
                let control_point = *geometry
                    .corners
                    .get(corner as usize)
                    .ok_or_else(|| anyhow!("FBX mesh '{}' has an invalid corner", mesh_name))?
                    as usize;
                let position = geometry
                    .position(corner as usize)
                    .ok_or_else(|| anyhow!("FBX mesh '{}' has a missing position", mesh_name))?;
                let position = [position[0] as f32, position[1] as f32, position[2] as f32];
                if position.iter().any(|coordinate| !coordinate.is_finite()) {
                    bail!("FBX mesh '{}' contains a non-finite vertex", mesh_name);
                }
                let index = u32::try_from(vertices.len())
                    .context("FBX mesh has more than u32::MAX triangle vertices")?;
                vertices.push(Vertex {
                    position: Vec3A::from_array(position),
                    color: Vec4::ONE,
                });
                control_points.push(control_point);
                indices.push(index);
            }
        }
    }
    Ok((vertices, indices, control_points))
}

pub fn load_animations_from_fbx(path: impl AsRef<Path>) -> Result<Vec<AnimationClip>> {
    let path = path.as_ref();
    let bytes = fs::read(path).with_context(|| format!("failed to read FBX {}", path.display()))?;
    let limits = fbx_reader::Limits::default();
    let scene = fbx_reader::Scene::parse(&bytes, &limits)
        .map_err(|error| anyhow!(error.to_string()))
        .with_context(|| format!("failed to parse FBX {}", path.display()))?;
    let hierarchy = Arc::new(scene.hierarchy);
    let animation = Arc::new(scene.animation);
    (0..animation.takes.len())
        .map(|index| AnimationClip::from_fbx(index, Arc::clone(&hierarchy), Arc::clone(&animation)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_fbx_files_report_parse_errors() {
        let path = std::env::temp_dir().join(format!("invalid-{}.fbx", std::process::id()));
        fs::write(&path, b"not an FBX file").unwrap();
        let result = load_animations_from_fbx(&path);
        fs::remove_file(&path).unwrap();
        assert!(
            result
                .err()
                .unwrap()
                .to_string()
                .contains("failed to parse FBX")
        );
    }

    #[test]
    fn loads_animation_takes_from_binary_fbx() {
        let bytes = fbx_reader::write::to_bytes(7400, &fbx_reader::write::skinned_scene(), true);
        let path = std::env::temp_dir().join(format!("animated-{}.fbx", std::process::id()));
        fs::write(&path, bytes).unwrap();
        let clips = load_animations_from_fbx(&path).unwrap();
        fs::remove_file(&path).unwrap();
        assert!(!clips.is_empty());
        assert!(clips.iter().all(|clip| !clip.name.trim().is_empty()));
    }

    #[test]
    fn triangulates_and_converts_fbx_mesh_geometry() {
        let bytes = fbx_reader::write::to_bytes(7400, &fbx_reader::write::skinned_scene(), true);
        let limits = fbx_reader::Limits::default();
        let scene = fbx_reader::Scene::parse(&bytes, &limits).unwrap();
        let instance = scene.mesh_instances().into_iter().next().unwrap();
        let geometry = scene.geometry(instance.geometry_id, &limits).unwrap();
        let (vertices, indices, control_points) = triangle_data(&geometry, &instance.name).unwrap();
        assert_eq!(vertices.len(), indices.len());
        assert_eq!(vertices.len(), control_points.len());
        assert!(!vertices.is_empty());
        assert!(vertices.iter().all(|vertex| vertex.position.is_finite()));
    }
}

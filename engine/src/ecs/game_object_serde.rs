use crate::graphics::Transform;
use anyhow::{Result, bail};
use cgmath::{InnerSpace, Matrix4, One, Quaternion, Vector3};
use serde::Deserialize;

#[derive(Deserialize)]
pub(crate) struct GameObjectDefinition {
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) transform: TransformDefinition,
    #[serde(default)]
    pub(crate) components: Vec<super::scene_serde::ComponentDefinition>,
    #[serde(default)]
    pub(crate) children: Vec<GameObjectDefinition>,
    #[serde(default)]
    pub(crate) references: std::collections::BTreeMap<String, serde_yaml::Value>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(default)]
pub struct TransformDefinition {
    #[serde(with = "vector3_serde")]
    pub translation: Vector3<f32>,
    #[serde(with = "quaternion_serde")]
    pub rotation: Quaternion<f32>,
    #[serde(with = "vector3_serde")]
    pub scale: Vector3<f32>,
}

impl TransformDefinition {
    pub fn to_transform(self) -> Result<Transform> {
        if !self.translation.x.is_finite()
            || !self.translation.y.is_finite()
            || !self.translation.z.is_finite()
            || !self.scale.x.is_finite()
            || !self.scale.y.is_finite()
            || !self.scale.z.is_finite()
            || !self.rotation.s.is_finite()
            || !self.rotation.v.x.is_finite()
            || !self.rotation.v.y.is_finite()
            || !self.rotation.v.z.is_finite()
        {
            bail!("game object transform values must be finite");
        }
        if self.scale.x.abs() <= f32::EPSILON
            || self.scale.y.abs() <= f32::EPSILON
            || self.scale.z.abs() <= f32::EPSILON
            || self.rotation.magnitude2() <= f32::EPSILON
        {
            bail!("game object scale and rotation must be non-zero");
        }
        let transform = Matrix4::from_translation(self.translation)
            * Matrix4::from(self.rotation.normalize())
            * Matrix4::from_nonuniform_scale(self.scale.x, self.scale.y, self.scale.z);
        use cgmath::SquareMatrix;
        if !is_finite_transform(transform) || transform.determinant().abs() <= f32::EPSILON {
            bail!("game object transforms must be finite and invertible");
        }
        Ok(transform)
    }
}

impl Default for TransformDefinition {
    fn default() -> Self {
        Self {
            translation: Vector3::new(0.0, 0.0, 0.0),
            rotation: Quaternion::one(),
            scale: Vector3::new(1.0, 1.0, 1.0),
        }
    }
}

pub(crate) fn is_finite_transform(transform: Transform) -> bool {
    [
        transform.x.x,
        transform.x.y,
        transform.x.z,
        transform.x.w,
        transform.y.x,
        transform.y.y,
        transform.y.z,
        transform.y.w,
        transform.z.x,
        transform.z.y,
        transform.z.z,
        transform.z.w,
        transform.w.x,
        transform.w.y,
        transform.w.z,
        transform.w.w,
    ]
    .iter()
    .all(|value| value.is_finite())
}

mod vector3_serde {
    use cgmath::Vector3;
    use serde::{Deserialize, Deserializer};

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vector3<f32>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let [x, y, z] = <[f32; 3]>::deserialize(deserializer)?;
        Ok(Vector3::new(x, y, z))
    }
}

mod quaternion_serde {
    use cgmath::Quaternion;
    use serde::{Deserialize, Deserializer};

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Quaternion<f32>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let [x, y, z, w] = <[f32; 4]>::deserialize(deserializer)?;
        Ok(Quaternion::new(w, x, y, z))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cgmath::SquareMatrix;

    #[test]
    fn transform_definition_deserializes_quaternion_and_builds_cgmath_matrix() {
        let definition: TransformDefinition = serde_yaml::from_str(
            "translation: [2.0, 0.0, 0.0]\nrotation: [0.0, 0.0, 0.70710677, 0.70710677]\nscale: [1.0, 2.0, 1.0]\n",
        )
        .unwrap();
        let transform = definition.to_transform().unwrap();
        assert!((transform.w.x - 2.0).abs() < 0.0001);
        assert!((transform.determinant().abs() - 2.0).abs() < 0.0001);
        assert!(
            serde_yaml::from_str::<TransformDefinition>("rotation: [0.0, 0.0, 0.0, 0.0]\n")
                .unwrap()
                .to_transform()
                .is_err()
        );
    }
}

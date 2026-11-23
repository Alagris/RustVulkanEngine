pub use engine::assets::Prefab;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asset_database::AssetDatabase;
    use crate::asset_database::file_nonce;
    use engine::ecs::{ComponentRegistry, Scene};
    use serde_yaml::Value;
    use std::fs;
    use std::path::PathBuf;

    fn temp_dir() -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "editor-prefab-{}-{}",
            std::process::id(),
            file_nonce()
        ));
        fs::create_dir_all(&directory).unwrap();
        directory
    }

    #[test]
    fn inherits_mapping_values_and_merges_id_keyed_children() {
        let root = temp_dir();
        fs::write(
            root.join("Base.prefab"),
            r#"
root:
  name: BaseCar
  transform:
    translation: [0.0, 0.0, 0.0]
  children:
    - id: wheel
      name: Wheel
      transform:
        translation: [1.0, 0.0, 0.0]
    - id: old-part
      name: OldPart
"#,
        )
        .unwrap();
        let base_source = fs::read_to_string(root.join("Base.prefab")).unwrap();
        fs::write(
            root.join("SportsCar.prefab"),
            r#"
parent: Base
root:
  name: SportsCar
  transform:
    translation: [5.0, 0.0, 0.0]
  children:
    - id: wheel
      transform:
        translation: [2.0, 0.0, 0.0]
    - id: old-part
      remove: true
    - id: trunk
      name: Trunk
"#,
        )
        .unwrap();

        let mut assets = AssetDatabase::scan(&root).unwrap();
        let prefab = assets.load_prefab("SportsCar").unwrap();
        assert_eq!(prefab.root["name"].as_str(), Some("SportsCar"));
        assert_eq!(
            prefab.root["transform"]["translation"][0].as_f64(),
            Some(5.0)
        );
        assert_eq!(prefab.root["children"].as_sequence().unwrap().len(), 2);
        assert_eq!(prefab.root["children"][0]["name"].as_str(), Some("Wheel"));
        assert_eq!(
            prefab.root["children"][0]["transform"]["translation"][0].as_f64(),
            Some(2.0)
        );
        assert_eq!(prefab.root["children"][1]["name"].as_str(), Some("Trunk"));

        let mut scene: Value = serde_yaml::from_str(
            r#"
root:
  name: World
  children:
    - prefab: SportsCar
      name: PlacedCar
      transform:
        translation: [10.0, 0.0, 0.0]
"#,
        )
        .unwrap();
        Prefab::expand_scene(&mut scene, &assets).unwrap();
        assert_eq!(
            scene["root"]["children"][0]["name"].as_str(),
            Some("PlacedCar")
        );
        assert_eq!(
            scene["root"]["children"][0]["children"][0]["name"].as_str(),
            Some("Wheel")
        );

        fs::write(
            root.join("Base.prefab"),
            base_source.replace("name: Wheel", "name: UpdatedWheel"),
        )
        .unwrap();
        assert_eq!(
            assets.load_prefab("SportsCar").unwrap().root["children"][0]["name"].as_str(),
            Some("Wheel"),
            "resolved prefab data should be served from the cache"
        );
        assets.clear_prefab_cache().unwrap();
        assert_eq!(
            assets.load_prefab("SportsCar").unwrap().root["children"][0]["name"].as_str(),
            Some("UpdatedWheel")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn instantiate_returns_a_scene_managed_game_object() {
        let root = temp_dir();
        fs::write(
            root.join("CameraRig.prefab"),
            "root:\n  name: CameraRig\n  components:\n    - type: Camera\n",
        )
        .unwrap();
        fs::write(
            root.join("world.scene"),
            "root:\n  name: World\n  children:\n    - prefab: CameraRig\n",
        )
        .unwrap();
        let assets = AssetDatabase::scan(&root).unwrap();
        let prefab = assets.load_prefab("CameraRig").unwrap();
        let expanded_scene = assets
            .prepare_development_scene(&root.join("world.scene"))
            .unwrap();
        let expanded_source = fs::read_to_string(&expanded_scene).unwrap();
        assert!(!expanded_source.contains("prefab:"));
        Scene::validate_yaml(&expanded_source).unwrap();
        let package_output = temp_dir();
        let packaged_scene = package_output.join("world.scene");
        assets
            .process_scene(&root.join("world.scene"), &packaged_scene)
            .unwrap();
        assert!(
            !fs::read_to_string(&packaged_scene)
                .unwrap()
                .contains("prefab:")
        );
        let registry = ComponentRegistry::new();
        let mut scene = Scene::from_yaml("root:\n  name: World\n", &registry).unwrap();
        scene.initialize().unwrap();

        let instance = prefab.instantiate(&mut scene, &registry).unwrap();
        assert_eq!(instance.borrow().name, "CameraRig");
        assert_eq!(instance.borrow().components(), 1);
        assert_eq!(scene.component_count(), 1);
        fs::remove_dir_all(expanded_scene.parent().unwrap()).unwrap();
        fs::remove_dir_all(package_output).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_cyclic_prefab_inheritance() {
        let root = temp_dir();
        fs::write(
            root.join("First.prefab"),
            "parent: Second\nroot:\n  name: First\n",
        )
        .unwrap();
        fs::write(
            root.join("Second.prefab"),
            "parent: First\nroot:\n  name: Second\n",
        )
        .unwrap();
        let error = AssetDatabase::scan(&root).err().unwrap();
        assert!(format!("{error:#}").contains("cyclic prefab inheritance"));
        fs::remove_dir_all(root).unwrap();
    }
}

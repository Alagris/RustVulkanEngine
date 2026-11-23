//! Integration tests driving the editor against the spoofed asset folders in
//! `tests/resources` through the `editor::testing` framework.

use editor::testing::{
    AssetDir, ProjectDir, assert_error_contains, project_error_at, scan_error_at,
};
use editor::{AssetRef, AssetType, RuntimeSettings};
use engine::data::animation_clip::AnimationClip;
use engine::ecs::{ComponentRegistry, Scene};
use engine::input::Input;
use std::fs;
use std::path::PathBuf;

fn reference(name: &str, asset_type: AssetType) -> AssetRef {
    AssetRef {
        name: name.to_owned(),
        asset_type,
    }
}

// ---------------------------------------------------------------------------
// Positive cases
// ---------------------------------------------------------------------------

#[test]
fn basic_project_scans_every_supported_asset_type() {
    let assets = AssetDir::fixture("basic_project/assets");
    let database = assets.scan_or_panic();
    for (name, asset_type) in [
        ("pipeline", AssetType::Pipeline),
        ("lit", AssetType::Material),
        ("LitShader", AssetType::Shader),
        ("main", AssetType::Scene),
        ("editor", AssetType::Scene),
        ("camera_rig", AssetType::Prefab),
        ("crate", AssetType::Prefab),
        ("Input/Controls", AssetType::Data),
        ("AnimationClip/Wave", AssetType::Data),
        ("readme", AssetType::Text),
    ] {
        let resolved = database.resolve(&reference(name, asset_type));
        assert!(
            resolved.is_ok(),
            "expected {asset_type:?} '{name}' to resolve: {resolved:?}"
        );
    }
    assert!(database.data_assets().get::<Input>("Controls").is_some());
    let triggers = database
        .data_assets()
        .get::<AnimationClip>("Wave")
        .unwrap()
        .with(|clip| clip.triggers.len());
    assert_eq!(triggers, Some(2));
}

#[test]
fn basic_project_loads_and_prepares_development_settings() {
    let project = ProjectDir::fixture("basic_project").load_or_panic();
    assert!(project.settings().runtime.pipeline.is_file());
    assert_eq!(
        project.settings().runtime.input_asset.as_deref(),
        Some("Controls")
    );

    let settings = project.prepare_development().unwrap();
    assert!(settings.pipeline.is_file());
    assert!(settings.default_scene.is_file());
    let expanded = fs::read_to_string(&settings.default_scene).unwrap();
    assert!(
        !expanded.contains("prefab:"),
        "development scenes must have all prefabs expanded"
    );
    assert!(expanded.contains("CameraRig"));
    assert!(expanded.contains("PlacedCrate"));

    let resolved_pipeline = fs::read_to_string(&settings.pipeline).unwrap();
    assert!(
        resolved_pipeline.contains("materials"),
        "development pipelines must point at resolved material copies"
    );
    for produced in [&settings.pipeline, &settings.default_scene] {
        fs::remove_dir_all(produced.parent().unwrap()).unwrap();
    }
}

#[test]
fn basic_project_packages_a_release_directory() {
    let project = ProjectDir::fixture("basic_project").load_or_panic();
    let output = AssetDir::temp("package-output");

    let runtime_settings_path = project.package(output.root()).unwrap();
    let runtime = RuntimeSettings::load(&runtime_settings_path).unwrap();
    assert!(runtime.pipeline.is_file());
    assert!(runtime.default_scene.is_file());

    let packaged_scene = fs::read_to_string(&runtime.default_scene).unwrap();
    assert!(!packaged_scene.contains("prefab:"));

    let material = fs::read_to_string(output.path("materials/lit.mat")).unwrap();
    assert!(material.contains("../shaders/LitShader.shader"));
    let shader = fs::read_to_string(output.path("shaders/LitShader.shader")).unwrap();
    assert!(
        !shader.contains("#include"),
        "packaged shaders must be fully preprocessed"
    );
    assert!(shader.contains("struct VertexInput"));

    assert!(output.path("data/data/controls.data").is_file());
    assert!(output.path("data/data/clips/wave.data").is_file());
    assert_eq!(runtime.input_asset.as_deref(), Some("data/controls.data"));
}

#[test]
fn camera_rig_prefab_instantiates_into_a_live_scene() {
    let database = AssetDir::fixture("basic_project/assets").scan_or_panic();
    let prefab = database.load_prefab("camera_rig").unwrap();
    let registry = ComponentRegistry::new();
    let mut scene = Scene::from_yaml("root:\n  name: World\n", &registry).unwrap();
    scene.initialize().unwrap();
    let instance = prefab.instantiate(&mut scene, &registry).unwrap();
    assert_eq!(instance.borrow().name, "CameraRig");
    assert_eq!(scene.component_count(), 1);
}

#[test]
fn prefab_inheritance_merges_overrides_removals_and_additions() {
    let database = AssetDir::fixture("prefab_inheritance").scan_or_panic();

    let sports_car = database.load_prefab("sports_car").unwrap();
    assert_eq!(sports_car.root["name"].as_str(), Some("SportsCar"));
    let children = sports_car.root["children"].as_sequence().unwrap();
    assert_eq!(children.len(), 2, "spare tire must be removed");
    assert_eq!(children[0]["name"].as_str(), Some("Wheel"));
    assert_eq!(
        children[0]["transform"]["translation"][0].as_f64(),
        Some(2.0),
        "wheel translation must be overridden by the child prefab"
    );
    assert_eq!(children[1]["name"].as_str(), Some("Spoiler"));

    let showroom = database.load_prefab("showroom_car").unwrap();
    assert_eq!(
        showroom.root["name"].as_str(),
        Some("SportsCar"),
        "prefabs with only a parent must inherit the full parent root"
    );

    let scene_path = database
        .resolve(&reference("garage", AssetType::Scene))
        .unwrap()
        .to_path_buf();
    let prepared = database.prepare_development_scene(&scene_path).unwrap();
    let expanded = fs::read_to_string(&prepared).unwrap();
    assert!(!expanded.contains("prefab:"));
    assert!(expanded.contains("ParkedCar"));
    assert!(expanded.contains("Spoiler"));
    fs::remove_dir_all(prepared.parent().unwrap()).unwrap();
}

#[test]
fn deeply_nested_assets_resolve_by_stem_and_type() {
    let database = AssetDir::fixture("deep_nesting").scan_or_panic();
    let scene = database
        .resolve(&reference("deep", AssetType::Scene))
        .unwrap();
    assert!(scene.ends_with(PathBuf::from("a/b/c/deep.scene")));
    let prefab = database
        .resolve(&reference("deep", AssetType::Prefab))
        .unwrap();
    assert!(prefab.ends_with(PathBuf::from("a/deep.prefab")));

    let other = database.load_prefab("other").unwrap();
    assert_eq!(
        other.root["children"][0]["name"].as_str(),
        Some("NestedInstance"),
        "prefab instances nested in prefabs must be expanded"
    );
}

#[test]
fn data_assets_inherit_parent_values() {
    let database = AssetDir::fixture("data_inheritance").scan_or_panic();
    let derived = database
        .data_assets()
        .get::<AnimationClip>("DerivedClip")
        .expect("derived clip must exist");
    assert_eq!(
        derived.with(|clip| clip.triggers.len()),
        Some(2),
        "derived clip must inherit the parent's triggers"
    );
}

#[test]
fn temp_copies_can_be_mutated_without_touching_fixtures() {
    let copy = AssetDir::temp_copy_of_fixture("prefab_inheritance");
    copy.remove("base_car.prefab");
    let error = copy.scan_error();
    assert!(
        error.contains("asset Prefab named 'base_car' was not found"),
        "unexpected error: {error}"
    );
    // The checked-in fixture must be unaffected by the mutation.
    AssetDir::fixture("prefab_inheritance").scan_or_panic();
}

#[test]
fn any_folder_can_be_used_as_an_asset_directory() {
    let scratch = AssetDir::temp("adhoc-assets");
    scratch.write("ad_hoc.scene", "root:\n  name: AdHoc\n");
    scratch.write("nested/dir/thing.prefab", "root:\n  name: Thing\n");
    let database = AssetDir::open(scratch.root()).scan_or_panic();
    assert_eq!(database.assets().count(), 2);
    assert!(
        database
            .resolve(&reference("thing", AssetType::Prefab))
            .is_ok()
    );
}

// ---------------------------------------------------------------------------
// Negative cases: scanning
// ---------------------------------------------------------------------------

#[test]
fn scan_rejects_missing_roots_and_non_directories() {
    let missing = std::env::temp_dir().join("editor-fixture-missing-root");
    let error = scan_error_at(&missing);
    assert!(
        error.contains("Failed to resolve asset directory"),
        "unexpected error: {error}"
    );

    let scratch = AssetDir::temp("not-a-directory");
    let file = scratch.write("file.txt", "not a directory");
    let error = scan_error_at(&file);
    assert!(
        error.contains("asset root is not a directory"),
        "unexpected error: {error}"
    );
}

#[test]
fn scan_rejects_cyclic_prefab_inheritance() {
    let error = AssetDir::fixture("invalid/cyclic_prefabs").scan_error();
    assert!(
        error.contains("cyclic prefab inheritance or instantiation"),
        "unexpected error: {error}"
    );
}

#[test]
fn scan_rejects_prefabs_instancing_themselves() {
    let error = AssetDir::fixture("invalid/self_instancing_prefab").scan_error();
    assert!(
        error.contains("cyclic prefab inheritance or instantiation"),
        "unexpected error: {error}"
    );
}

#[test]
fn scan_rejects_scenes_referencing_unknown_prefabs() {
    let error = AssetDir::fixture("invalid/scene_missing_prefab").scan_error();
    assert!(
        error.contains("Failed to instantiate prefab 'ghost'"),
        "unexpected error: {error}"
    );
    assert!(
        error.contains("asset Prefab named 'ghost' was not found"),
        "unexpected error: {error}"
    );
}

#[test]
fn duplicate_scene_names_require_path_resolution() {
    let database = AssetDir::fixture("invalid/duplicate_scenes").scan_or_panic();
    assert!(
        database
            .resolve(&reference("world", AssetType::Scene))
            .unwrap_err()
            .to_string()
            .contains("ambiguous Scene")
    );
}

#[test]
fn scan_rejects_prefabs_with_unsupported_fields() {
    let error = AssetDir::fixture("invalid/prefab_unknown_field").scan_error();
    assert!(
        error.contains("contains unsupported prefab fields"),
        "unexpected error: {error}"
    );
    assert!(error.contains("physics"), "unexpected error: {error}");
    assert!(error.contains("gravity"), "unexpected error: {error}");
}

#[test]
fn scan_rejects_prefabs_without_root_or_parent() {
    let error = AssetDir::fixture("invalid/prefab_without_root").scan_error();
    assert!(
        error.contains("must define root or inherit a parent prefab"),
        "unexpected error: {error}"
    );
}

#[test]
fn scan_rejects_scenes_without_a_root() {
    let error = AssetDir::fixture("invalid/scene_without_root").scan_error();
    assert!(
        error.contains("scene must contain a root game object"),
        "unexpected error: {error}"
    );
}

#[test]
fn scan_rejects_scenes_with_zero_scale_transforms() {
    let error = AssetDir::fixture("invalid/scene_zero_scale").scan_error();
    assert!(
        error.contains("game object scale and rotation must be non-zero"),
        "unexpected error: {error}"
    );
}

#[test]
fn scan_rejects_scenes_with_blank_object_names() {
    let error = AssetDir::fixture("invalid/scene_blank_name").scan_error();
    assert!(
        error.contains("game object names must not be empty"),
        "unexpected error: {error}"
    );
}

#[test]
fn scan_rejects_unparseable_scene_yaml() {
    let error = AssetDir::fixture("invalid/scene_invalid_yaml").scan_error();
    assert!(
        error.contains("Invalid YAML in"),
        "unexpected error: {error}"
    );
    assert!(error.contains("broken.scene"), "unexpected error: {error}");
}

#[cfg(debug_assertions)]
#[test]
fn scan_rejects_duplicate_unique_components() {
    let error = AssetDir::fixture("invalid/duplicate_unique_component").scan_error();
    assert!(
        error.contains("more than one unique component 'Camera'"),
        "unexpected error: {error}"
    );
}

#[cfg(debug_assertions)]
#[test]
fn scan_rejects_missing_required_components() {
    let error = AssetDir::fixture("invalid/missing_required_component").scan_error();
    assert!(
        error.contains("requires component 'SkinnedMesh'"),
        "unexpected error: {error}"
    );
}

#[test]
fn scan_rejects_unregistered_data_asset_types() {
    let error = AssetDir::fixture("invalid/data_unregistered_type").scan_error();
    assert!(
        error.contains("unregistered data asset type 'Teleporter'"),
        "unexpected error: {error}"
    );
}

#[test]
fn scan_rejects_cyclic_data_asset_parents() {
    let error = AssetDir::fixture("invalid/data_cyclic_parents").scan_error();
    assert!(
        error.contains("data asset inheritance cycle detected"),
        "unexpected error: {error}"
    );
}

#[test]
fn scan_rejects_shaders_without_a_declared_name() {
    let error = AssetDir::fixture("invalid/shader_bad_declaration").scan_error();
    assert!(
        error.contains("Failed to read declared shader name"),
        "unexpected error: {error}"
    );
    assert!(error.contains("broken.shader"), "unexpected error: {error}");
}

// ---------------------------------------------------------------------------
// Negative cases: pipeline preparation and packaging
// ---------------------------------------------------------------------------

#[test]
fn development_preparation_rejects_pipelines_without_passes() {
    let assets = AssetDir::fixture("pipelines/missing_passes");
    let database = assets.scan_or_panic();
    let error = database
        .prepare_development_pipeline(&assets.path("pipeline.pipe"))
        .unwrap_err();
    assert_error_contains(&error, "pipeline must contain a passes sequence");
}

#[test]
fn development_preparation_rejects_unknown_materials() {
    let assets = AssetDir::fixture("pipelines/unknown_material");
    let database = assets.scan_or_panic();
    let error = database
        .prepare_development_pipeline(&assets.path("pipeline.pipe"))
        .unwrap_err();
    assert_error_contains(&error, "asset Material named 'ghost' was not found");
}

#[test]
fn development_preparation_rejects_materials_without_shader_entries() {
    let assets = AssetDir::fixture("pipelines/material_without_shader");
    let database = assets.scan_or_panic();
    let error = database
        .prepare_development_pipeline(&assets.path("pipeline.pipe"))
        .unwrap_err();
    assert_error_contains(&error, "material must name a shader asset");
}

#[test]
fn development_preparation_rejects_unknown_shaders() {
    let assets = AssetDir::fixture("pipelines/unknown_shader");
    let database = assets.scan_or_panic();
    let error = database
        .prepare_development_pipeline(&assets.path("pipeline.pipe"))
        .unwrap_err();
    assert_error_contains(&error, "asset Shader named 'DoesNotExist' was not found");
}

#[test]
fn packaging_rejects_unknown_materials_too() {
    let assets = AssetDir::fixture("pipelines/unknown_material");
    let database = assets.scan_or_panic();
    let output = AssetDir::temp("package-invalid-output");
    let error = database
        .package(&assets.path("pipeline.pipe"), output.root())
        .unwrap_err();
    assert_error_contains(&error, "asset Material named 'ghost' was not found");
}

// ---------------------------------------------------------------------------
// Negative cases: project settings
// ---------------------------------------------------------------------------

#[test]
fn project_load_rejects_settings_referencing_missing_scenes() {
    let error = ProjectDir::fixture("invalid_projects/missing_scene").load_error();
    assert!(
        error.contains("asset Scene named 'ghost' was not found"),
        "unexpected error: {error}"
    );
}

#[test]
fn project_load_rejects_settings_assets_with_wrong_extensions() {
    let error = ProjectDir::fixture("invalid_projects/wrong_extension").load_error();
    assert!(
        error.contains("must have the .pipe extension"),
        "unexpected error: {error}"
    );
}

#[test]
fn project_load_rejects_missing_settings_files() {
    let scratch = AssetDir::temp("empty-project");
    let error = project_error_at(scratch.path("settings.yaml"));
    assert!(
        error.contains("Failed to resolve settings file"),
        "unexpected error: {error}"
    );
}

#[test]
fn project_load_rejects_blank_settings_entries() {
    let project = ProjectDir::temp("blank-settings");
    project.write_asset("readme.txt", "placeholder");
    project.write_settings("pipeline: \"\"\ndefault_scene: \"\"\neditor_scene: \"\"\n");
    let error = project.load_error();
    assert!(
        error.contains("settings asset name must not be empty"),
        "unexpected error: {error}"
    );
}

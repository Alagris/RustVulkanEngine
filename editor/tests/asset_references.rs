use editor::testing::{AssetDir, assert_error_contains, resources_root};
use editor::{AssetRef, AssetType};
use engine::assets::{AssetStore, DataAssetFile, Prefab, PrefabRef, UiDocument, UiDocumentRef};
use engine::data::{DataRef, terrain_layer::TerrainLayer};
use engine::ecs::{ComponentRegistry, Foliage, Scene, Terrain};
use engine::ui::elem::{List, Table};
use std::fs;

fn assert_landscape(assets: &AssetStore) -> (PrefabRef, UiDocumentRef, DataRef<TerrainLayer>) {
    let registry = ComponentRegistry::new();
    let mut scene = Scene::from_yaml("root: {name: World}", &registry).unwrap();
    scene.initialize().unwrap();
    let prefab = assets.get::<Prefab>("main.prefab").unwrap();
    let object = prefab
        .with(|prefab| prefab.instantiate(&mut scene, &registry))
        .unwrap()
        .unwrap();
    let object = object.borrow();
    assert_eq!(object.name, "Landscape");
    assert_eq!(object.components(), 2);
    assert!(
        object.children().is_empty(),
        "foliage is asset wiring only, not a scatter implementation"
    );
    let prefabs = {
        let foliage = object.get_component::<Foliage>(&scene).unwrap();
        let serialized = registry.serialize_component(&*foliage).unwrap();
        assert_eq!(
            serialized["prefabs"][0]["asset"]["path"],
            "plants/oak.prefab"
        );
        foliage.prefabs.clone()
    };
    assert_eq!(prefabs.len(), 3);
    for (reference, expected) in prefabs.iter().zip(["Oak", "Pine", "SnowPine"]) {
        assert!(reference.is_alive());
        let plant = reference
            .with(|prefab| prefab.instantiate(&mut scene, &registry))
            .unwrap()
            .unwrap();
        assert_eq!(plant.borrow().name, expected);
    }
    let terrain = object.get_component::<Terrain>(&scene).unwrap();
    assert!(!terrain.layer.is_nested());
    assert_eq!(
        terrain.layer.with(|layer| layer.label.clone()).as_deref(),
        Some("Forest floor")
    );
    let ui = object.references["palette"].typed::<UiDocument>().unwrap();
    let mut document = ui.with(UiDocument::instantiate).unwrap().unwrap();
    let list_id = document.find_id("foliage")[0];
    let table_id = document.find_id("layers")[0];
    let row = document
        .element_as::<List>(list_id)
        .unwrap()
        .template
        .as_ref()
        .unwrap()
        .clone();
    let cell = document
        .element_as::<Table>(table_id)
        .unwrap()
        .template
        .as_ref()
        .unwrap();
    assert_eq!(row.path(), "ui/templates/row.ui");
    assert_eq!(cell.path(), "ui/templates/cell.ui");
    let overridden = document
        .element_as::<List>(document.find_id("overridden-list")[0])
        .unwrap()
        .template
        .as_ref()
        .unwrap();
    assert_eq!(
        overridden.path(),
        cell.path(),
        "include overrides must resolve templates in development too"
    );
    assert!(
        document.children(list_id).unwrap().is_empty(),
        "templates must not be eagerly injected"
    );
    assert!(document.children(table_id).unwrap().is_empty());
    let mut row_instance = row.with(UiDocument::instantiate).unwrap().unwrap();
    let second_row = row.with(UiDocument::instantiate).unwrap().unwrap();
    let caption = row_instance.find_id("caption")[0];
    row_instance.set_text(caption, "Changed").unwrap();
    assert_eq!(
        second_row
            .element(second_row.find_id("caption")[0])
            .unwrap()
            .text(),
        Some("Foliage row")
    );
    assert_eq!(
        row_instance.references(row_instance.root()).unwrap()["owner"].path(),
        ui.path(),
        "weak UI cycles must remain references, not includes"
    );
    assert_eq!(
        document
            .references(document.find_id("included")[0])
            .unwrap()["pine"]
            .path(),
        "variants/pine.prefab"
    );
    assert_eq!(
        document.references(document.root()).unwrap()["ground"]
            .typed::<DataAssetFile>()
            .unwrap()
            .with(|file| file.with::<TerrainLayer, _>(|layer| layer.label.clone()))
            .unwrap()
            .unwrap(),
        "Forest floor"
    );
    assets
        .with(|| document.set_attribute(table_id, "template", row.path()))
        .unwrap();
    assert_eq!(
        document
            .element_as::<Table>(table_id)
            .unwrap()
            .template
            .as_ref()
            .unwrap()
            .path(),
        row.path()
    );
    let error = assets
        .with(|| document.set_attribute(table_id, "template", "missing.ui"))
        .unwrap_err();
    assert_error_contains(&error, "missing.ui");
    assert_eq!(
        document
            .element_as::<Table>(table_id)
            .unwrap()
            .template
            .as_ref()
            .unwrap()
            .path(),
        row.path(),
        "a failed attribute update must preserve the previous template"
    );
    let layer = assets
        .data_assets()
        .get::<TerrainLayer>("layers/ground.data")
        .unwrap();
    (prefab, row, layer)
}

#[test]
fn component_and_ui_fixture_references_are_resolved_weak_and_independent() {
    let directory = AssetDir::fixture("references/valid");
    let database = directory.scan_or_panic();
    let (prefab, row, layer) = database
        .with_runtime_assets(|assets| Ok(assert_landscape(assets)))
        .unwrap();
    let scene_path = database
        .prepare_development_scene(&directory.path("world.scene"))
        .unwrap();
    let scene = database
        .with_runtime_assets(|assets| assets.load_scene(&scene_path, &ComponentRegistry::new()))
        .unwrap();
    let root = scene.root.borrow();
    assert_eq!(root.children().len(), 3);
    let nested = root.children()[1].borrow();
    let terrain = nested.get_component::<Terrain>(&scene).unwrap();
    assert!(terrain.layer.is_nested());
    assert_eq!(
        terrain.layer.with(|layer| layer.label.clone()).as_deref(),
        Some("Nested layer")
    );
    let snow = root.children()[2].borrow();
    assert_eq!(
        snow.get_component::<Terrain>(&scene)
            .unwrap()
            .layer
            .with(|layer| layer.label.clone())
            .as_deref(),
        Some("Snow")
    );
    fs::remove_dir_all(scene_path.parent().unwrap()).unwrap();
    drop(database);
    assert!(!prefab.is_alive());
    assert!(!row.is_alive());
    assert!(!layer.is_alive());
    assert!(
        terrain.layer.is_alive(),
        "inline assets belong to their component, not to the database"
    );
}

#[test]
fn packaged_reference_fixtures_load_without_editor_or_original_source_files() {
    let output = AssetDir::temp("reference-package");
    let directory = AssetDir::temp_copy_of_fixture("references/valid");
    {
        let database = directory.scan_or_panic();
        database
            .with_runtime_assets(|assets| assets.package(output.root()))
            .unwrap();
        let scene = database
            .prepare_development_scene(&directory.path("world.scene"))
            .unwrap();
        fs::copy(&scene, output.path("world.scene")).unwrap();
        fs::remove_dir_all(scene.parent().unwrap()).unwrap();
    }
    drop(directory);
    let assets = AssetStore::load(output.root()).unwrap();
    assert_landscape(&assets);
    let scene = assets
        .load_scene("world.scene", &ComponentRegistry::new())
        .unwrap();
    assert_eq!(scene.root.borrow().children().len(), 3);
    let packaged = fs::read_to_string(output.path("main.prefab")).unwrap();
    assert!(packaged.contains("path: plants/oak.prefab"));
    assert!(packaged.contains("ref: layers/ground.data"));
    assert!(!packaged.contains("ref: Ground"));
    let ui = fs::read_to_string(output.path("ui/palette.ui")).unwrap();
    assert!(!ui.contains("<Include"));
    assert!(ui.contains("template=\"ui/templates/row.ui\""));
}

#[test]
fn duplicate_names_are_disambiguated_by_prefab_data_and_ui_paths() {
    let directory = AssetDir::fixture("references/duplicates");
    let database = directory.scan_or_panic();
    for (name, asset_type) in [
        ("pine", AssetType::Prefab),
        ("row", AssetType::Ui),
        ("TerrainLayer/Ground", AssetType::Data),
    ] {
        let error = database
            .resolve(&AssetRef {
                name: name.into(),
                asset_type,
            })
            .unwrap_err();
        assert_error_contains(&error, "ambiguous");
    }
    database
        .with_runtime_assets(|assets| {
            let registry = ComponentRegistry::new();
            let mut scene = Scene::from_yaml("root: {name: World}", &registry)?;
            let object = assets
                .get::<Prefab>("main.prefab")?
                .with(|prefab| prefab.instantiate(&mut scene, &registry))??;
            let object = object.borrow();
            assert_eq!(
                object
                    .get_component::<Terrain>(&scene)
                    .unwrap()
                    .layer
                    .with(|layer| layer.label.clone())
                    .as_deref(),
                Some("Desert")
            );
            let foliage = object.get_component::<Foliage>(&scene).unwrap();
            assert_eq!(foliage.prefabs.len(), 2);
            assert_ne!(foliage.prefabs[0].path(), foliage.prefabs[1].path());
            for (reference, expected) in foliage.prefabs.iter().zip(["Pine", "SnowPine"]) {
                assert_eq!(
                    reference.with(|prefab| prefab.root["name"].as_str().unwrap().to_owned())?,
                    expected
                );
            }
            let document = assets
                .get::<UiDocument>("ui/palette.ui")?
                .with(UiDocument::instantiate)??;
            let first = document
                .element_as::<List>(document.find_id("first")[0])
                .unwrap()
                .template
                .as_ref()
                .unwrap();
            let second = document
                .element_as::<Table>(document.find_id("second")[0])
                .unwrap()
                .template
                .as_ref()
                .unwrap();
            assert_ne!(first.path(), second.path());
            let first = first.with(UiDocument::instantiate)??;
            assert_eq!(first.element(first.root()).unwrap().text(), Some("Forest"));
            let second = second.with(UiDocument::instantiate)??;
            assert_eq!(
                second.element(second.root()).unwrap().text(),
                Some("Desert")
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn invalid_reference_resource_cases_report_the_expected_errors() {
    let root = resources_root().join("references").join("invalid");
    let mut cases = fs::read_dir(&root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_dir())
        .collect::<Vec<_>>();
    cases.sort();
    let mut tested = 0;
    for case in cases {
        let expected = fs::read_to_string(case.join("expected_error.txt")).unwrap();
        assert!(!expected.trim().is_empty(), "{}", case.display());
        let result = AssetDir::open(&case).scan().and_then(|database| {
            let prefab = database.load_prefab("main")?;
            let registry = ComponentRegistry::new();
            let mut scene = Scene::from_yaml("root: {name: World}", &registry)?;
            prefab.instantiate(&mut scene, &registry)?;
            Ok(())
        });
        let error = result.expect_err(&format!("{} should fail", case.display()));
        assert_error_contains(&error, expected.trim());
        tested += 1;
    }
    assert_eq!(
        tested, 23,
        "reference fixture directories must not be skipped"
    );
}

#[test]
fn rebuilding_prefab_cache_replaces_the_matching_data_database() {
    let directory = AssetDir::temp_copy_of_fixture("references/valid");
    let mut database = directory.scan_or_panic();
    let old = database
        .data_assets()
        .get::<TerrainLayer>("Ground")
        .unwrap();
    let source = fs::read_to_string(directory.path("layers/ground.data")).unwrap();
    directory.write(
        "layers/ground.data",
        &source.replace("Forest floor", "Updated floor"),
    );
    database.clear_prefab_cache().unwrap();
    assert!(!old.is_alive());
    assert_eq!(
        database
            .data_assets()
            .get::<TerrainLayer>("Ground")
            .unwrap()
            .with(|layer| layer.label.clone())
            .as_deref(),
        Some("Updated floor")
    );
    let registry = ComponentRegistry::new();
    let mut scene = Scene::from_yaml("root: {name: World}", &registry).unwrap();
    let object = database
        .load_prefab("main")
        .unwrap()
        .instantiate(&mut scene, &registry)
        .unwrap();
    assert_eq!(
        object
            .borrow()
            .get_component::<Terrain>(&scene)
            .unwrap()
            .layer
            .with(|layer| layer.label.clone())
            .as_deref(),
        Some("Updated floor")
    );
}

#[test]
fn explicit_pipeline_dependency_paths_preserve_distinct_assets_when_packaged() {
    let directory = AssetDir::fixture("references/valid");
    let database = directory.scan_or_panic();
    let output = AssetDir::temp("path-based-pipeline");
    let prepared = database
        .prepare_development_pipeline(&directory.path("pipeline.pipe"))
        .unwrap();
    let packaged = database
        .package(&directory.path("pipeline.pipe"), output.root())
        .unwrap();
    for (pipeline, development) in [(prepared.as_path(), true), (packaged.as_path(), false)] {
        let value: serde_yaml::Value =
            serde_yaml::from_str(&fs::read_to_string(pipeline).unwrap()).unwrap();
        let passes = value["passes"].as_sequence().unwrap();
        assert_eq!(passes.len(), 2);
        assert_ne!(passes[0]["material"], passes[1]["material"]);
        for (pass, expected) in passes.iter().zip(["ForestShader", "SnowShader"]) {
            let material = pipeline
                .parent()
                .unwrap()
                .join(pass["material"].as_str().unwrap());
            assert!(material.is_file(), "{}", material.display());
            let value: serde_yaml::Value =
                serde_yaml::from_str(&fs::read_to_string(&material).unwrap()).unwrap();
            let shader = material
                .parent()
                .unwrap()
                .join(value["material"]["shader"].as_str().unwrap());
            assert!(shader.is_file(), "{}", shader.display());
            assert!(fs::read_to_string(&shader).unwrap().contains(expected));
            if !development {
                assert!(
                    shader
                        .canonicalize()
                        .unwrap()
                        .starts_with(output.root().canonicalize().unwrap())
                );
            }
        }
    }
    fs::remove_dir_all(prepared.parent().unwrap()).unwrap();
}

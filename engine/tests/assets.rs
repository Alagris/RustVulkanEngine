use engine::assets::{
    AssetKind, AssetNameResolver, AssetStore, DataAssetFile, Prefab, PrefabRef, UiDocument,
    UiDocumentRef,
};
use engine::data::DataRef;
use engine::ecs::{ComponentRegistry, Scene};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize, Serialize, engine::data::DataAsset)]
struct AssetProbe {
    value: u32,
    #[serde(default)]
    other: Option<DataRef<AssetProbe>>,
    #[serde(default)]
    prefab: Option<PrefabRef>,
}

struct TestDir(PathBuf);
impl TestDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "engine-asset-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn write(&self, path: &str, source: &str) {
        let path = self.0.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, source).unwrap();
    }
}
impl Drop for TestDir {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn basic(dir: &TestDir) {
    dir.write("actors/player.prefab", "root:\n  name: Player\n  references:\n    self: {asset: {type: prefab, path: actors/player.prefab}}\n    ui: {asset: {type: ui, path: ui/player.ui}}\n    data: {asset: {type: data, path: stats/player.data}}\n");
    dir.write("ui/player.ui", "<Canvas id='root'><Reference key='actor' type='prefab' path='actors/player.prefab'/><Label id='label'>Player</Label></Canvas>");
    dir.write("stats/player.data", "type: AssetProbe\ndata:\n  value: 42\n  other: {ref: stats/player.data}\n  prefab: {asset: {type: prefab, path: actors/player.prefab}}\n");
}

#[test]
fn runtime_references_are_resolved_weak_and_instantiate_without_editor() {
    fn send_sync<T: Send + Sync>() {}
    send_sync::<PrefabRef>();
    let dir = TestDir::new();
    basic(&dir);
    let assets = AssetStore::load(&dir.0).unwrap();
    let prefab = assets.get::<Prefab>("actors\\player.prefab").unwrap();
    let registry = ComponentRegistry::new();
    let mut scene = Scene::from_yaml("root:\n  name: World", &registry).unwrap();
    let object = prefab
        .with(|prefab| prefab.instantiate(&mut scene, &registry))
        .unwrap()
        .unwrap();
    assert_eq!(object.borrow().name, "Player");
    assert!(
        object.borrow().children().is_empty(),
        "weak references must not be eagerly instanced"
    );
    let references = object.borrow().references.clone();
    assert_eq!(references.len(), 3);
    let self_reference = references["self"].typed::<Prefab>().unwrap();
    assert_eq!(self_reference.path(), prefab.path());
    let ui = references["ui"].typed::<UiDocument>().unwrap();
    let mut document = ui.with(UiDocument::instantiate).unwrap().unwrap();
    assert_eq!(document.find_id("label").len(), 1);
    let doc_ref = document.references(document.root()).unwrap()["actor"].clone();
    assert_eq!(doc_ref.path(), "actors/player.prefab");
    let second = ui.with(UiDocument::instantiate).unwrap().unwrap();
    document
        .set_text(document.find_id("label")[0], "changed")
        .unwrap();
    assert_eq!(
        second.element(second.find_id("label")[0]).unwrap().text(),
        Some("Player")
    );
    let data = references["data"].typed::<DataAssetFile>().unwrap();
    assert_eq!(
        data.with(|data| data.with::<AssetProbe, _>(|data| data.value))
            .unwrap()
            .unwrap(),
        42
    );
    assert_eq!(
        assets
            .data_assets()
            .get::<AssetProbe>("stats/player.data")
            .unwrap()
            .with(|data| data.other.as_ref().unwrap().with(|other| other.value)),
        Some(Some(42))
    );
    let serialized = serde_yaml::to_string(&prefab).unwrap();
    assert!(serialized.contains("path: actors/player.prefab"));
    assert!(!serialized.contains("name:"));
    drop(assets);
    assert!(!prefab.is_alive());
    assert!(!doc_ref.is_alive());
    assert!(!data.is_alive());
    assert!(
        self_reference
            .with(|_| ())
            .unwrap_err()
            .to_string()
            .contains("expired")
    );
}

#[test]
fn packaged_assets_round_trip_and_have_no_original_file_dependency() {
    let dir = TestDir::new();
    let output = TestDir::new();
    basic(&dir);
    let assets = AssetStore::load(&dir.0).unwrap();
    assets.package(&output.0).unwrap();
    drop(assets);
    drop(dir);
    let assets = AssetStore::load(&output.0).unwrap();
    let probe = assets.get::<DataAssetFile>("stats/player.data").unwrap();
    assert!(
        probe
            .with(|file| file.with::<AssetProbe, _>(|data| data
                .prefab
                .as_ref()
                .unwrap()
                .is_alive()))
            .unwrap()
            .unwrap()
    );
    let ui = assets.get::<UiDocument>("ui/player.ui").unwrap();
    assert_eq!(
        ui.with(UiDocument::instantiate)
            .unwrap()
            .unwrap()
            .find_id("label")
            .len(),
        1
    );
}

#[test]
fn path_only_loading_rejects_names_wrong_types_and_escaping_paths() {
    let dir = TestDir::new();
    basic(&dir);
    let assets = AssetStore::load(&dir.0).unwrap();
    for path in [
        "../actors/player.prefab",
        "",
        "C:\\outside.prefab",
        "/outside.prefab",
        "missing.prefab",
    ] {
        assert!(assets.get::<Prefab>(path).is_err(), "{path}");
    }
    assert!(
        assets
            .get::<Prefab>("ui/player.ui")
            .unwrap_err()
            .to_string()
            .contains("expected")
    );
    let value = serde_yaml::from_str("{asset: {type: prefab, name: player}}").unwrap();
    assert!(
        format!("{:#}", assets.deserialize::<PrefabRef>(value).unwrap_err())
            .contains("development-only")
    );
    let value = serde_yaml::from_str("{asset: {type: ui, path: actors/player.prefab}}").unwrap();
    assert!(assets.deserialize::<UiDocumentRef>(value).is_err());
    dir.write("bad.prefab", "parent: player\nroot: {name: Bad}");
    assert!(
        AssetStore::load(&dir.0)
            .err()
            .unwrap()
            .to_string()
            .contains("bad.prefab")
    );
    fs::remove_file(dir.0.join("bad.prefab")).unwrap();
    dir.write(
        "stats/bad.data",
        "type: AssetProbe\ndata:\n  value: 0\n  other: {ref: player}\n",
    );
    assert!(format!("{:#}", AssetStore::load(&dir.0).err().unwrap()).contains("development-only"));
}

struct Names<'a>(&'a Path);
impl AssetNameResolver for Names<'_> {
    fn resolve_asset_name(&self, name: &str, kind: AssetKind) -> anyhow::Result<PathBuf> {
        match (name, kind) {
            ("Player", AssetKind::Prefab) => Ok(self.0.join("actors/player.prefab")),
            _ => anyhow::bail!("unknown name '{name}'"),
        }
    }
}

#[test]
fn development_names_are_compiled_to_paths_before_export() {
    let dir = TestDir::new();
    let output = TestDir::new();
    dir.write("actors/player.prefab", "root: {name: Player}");
    dir.write("other.prefab", "parent: Player\nroot:\n  name: Other\n  references:\n    target: {asset: {type: prefab, name: Player}}\n");
    dir.write(
        "view.ui",
        "<Canvas><Reference key='target' type='prefab' name='Player'/></Canvas>",
    );
    dir.write(
        "base.data",
        "type: AssetProbe\nname: Base\ndata: {value: 1}",
    );
    dir.write(
        "child.data",
        "type: AssetProbe\nparent: Base\ndata: {other: {ref: Base}}",
    );
    let assets = AssetStore::load_development(&dir.0, &Names(&dir.0)).unwrap();
    let child = assets
        .data_assets()
        .get::<AssetProbe>("child.data")
        .unwrap();
    assert_eq!(child.with(|child| child.value), Some(1));
    assets.package(&output.0).unwrap();
    let prefab = fs::read_to_string(output.0.join("other.prefab")).unwrap();
    assert!(!prefab.contains("parent:"));
    assert!(!prefab.contains("name: Player"));
    assert!(prefab.contains("path: actors/player.prefab"));
    assert!(
        fs::read_to_string(output.0.join("child.data"))
            .unwrap()
            .contains("ref: base.data")
    );
    assert!(AssetStore::load(&output.0).is_ok());
}

#[test]
fn inheritance_cycles_are_errors_but_weak_cycles_are_allowed() {
    let dir = TestDir::new();
    dir.write("a.prefab", "parent: b.prefab\nroot: {name: A}");
    dir.write("b.prefab", "parent: a.prefab\nroot: {name: B}");
    assert!(format!("{:#}", AssetStore::load(&dir.0).err().unwrap()).contains("cyclic prefab"));
    dir.write(
        "a.prefab",
        "root:\n  name: A\n  references:\n    b: {asset: {type: prefab, path: b.prefab}}",
    );
    dir.write(
        "b.prefab",
        "root:\n  name: B\n  references:\n    a: {asset: {type: prefab, path: a.prefab}}",
    );
    assert!(AssetStore::load(&dir.0).is_ok());
}

#[test]
fn data_paths_normalize_windows_separators_in_typed_and_dynamic_links() {
    let dir = TestDir::new();
    dir.write("modifiers/tap.data", "type: Tap\ndata: {}");
    let assets = AssetStore::load(&dir.0).unwrap();
    assert!(
        assets
            .data_assets()
            .try_get::<engine::input::Tap>("modifiers\\tap.data")
            .unwrap()
            .unwrap()
            .is_alive()
    );
    let value = serde_yaml::from_str("ref: 'modifiers\\tap.data'").unwrap();
    let modifier = assets
        .deserialize::<DataRef<dyn engine::input::Modifier>>(value)
        .unwrap();
    assert!(modifier.with(|_| true).unwrap());
    assert!(
        format!(
            "{:#}",
            assets
                .data_assets()
                .try_get::<engine::input::Tap>("tap")
                .unwrap_err()
        )
        .contains("development-only")
    );
    let value = serde_yaml::from_str("ref: 'modifiers\\..\\outside.data'").unwrap();
    let error = assets
        .deserialize::<DataRef<dyn engine::input::Modifier>>(value)
        .unwrap_err();
    assert!(format!("{error:#}").contains("cannot contain '..'"));
}

#[test]
fn runtime_loading_rejects_each_development_only_reference_form() {
    let cases = [
        ("main.prefab", "parent: Tree\nroot: {name: Test}"),
        (
            "main.prefab",
            "root: {name: Test, children: [{prefab: Tree}]}",
        ),
        (
            "main.prefab",
            "root: {name: Test, references: {tree: {asset: {type: prefab, name: Tree}}}}",
        ),
        (
            "main.prefab",
            "root: {name: Test, components: [{type: Terrain, data: {layer: {ref: Ground}}}]}",
        ),
        (
            "main.ui",
            "<Canvas><Reference key='tree' type='prefab' name='Tree'/></Canvas>",
        ),
        ("main.ui", "<List template='Row'/>"),
        ("main.ui", "<Table template='Cell'/>"),
        (
            "main.data",
            "type: AssetProbe\nparent: Base\ndata: {value: 1}",
        ),
        (
            "main.data",
            "type: AssetProbe\ndata: {value: 1, other: {ref: Other}}",
        ),
    ];
    for (path, source) in cases {
        let dir = TestDir::new();
        dir.write(path, source);
        let error = AssetStore::load(&dir.0).err().expect(source);
        assert!(
            format!("{error:#}").contains("development-only"),
            "{source}\n{error:#}"
        );
    }
}

#[test]
fn nested_loading_contexts_are_restored_after_errors() {
    let first = TestDir::new();
    let second = TestDir::new();
    first.write("same.prefab", "root: {name: First}");
    second.write("same.prefab", "root: {name: Second}");
    let first = AssetStore::load(&first.0).unwrap();
    let second = AssetStore::load(&second.0).unwrap();
    let value =
        serde_yaml::from_str::<serde_yaml::Value>("{asset: {type: prefab, path: same.prefab}}")
            .unwrap();
    first
        .with(|| {
            let reference = second.deserialize::<PrefabRef>(value.clone())?;
            assert_eq!(
                reference.with(|prefab| prefab.root["name"].clone())?,
                "Second"
            );
            let invalid = serde_yaml::from_str::<serde_yaml::Value>(
                "{asset: {type: prefab, path: missing.prefab}}",
            )?;
            assert!(second.deserialize::<PrefabRef>(invalid).is_err());
            let restored = serde_yaml::from_value::<PrefabRef>(value.clone())?;
            assert_eq!(
                restored.with(|prefab| prefab.root["name"].clone())?,
                "First"
            );
            Ok(())
        })
        .unwrap();
    let error = serde_yaml::from_value::<PrefabRef>(value).unwrap_err();
    assert!(error.to_string().contains("AssetStore loading context"));
}

#[test]
fn data_file_wrappers_do_not_share_ownership_with_the_data_database() {
    let dir = TestDir::new();
    basic(&dir);
    let assets = AssetStore::load(&dir.0).unwrap();
    let data = assets.get::<DataAssetFile>("stats/player.data").unwrap();
    let linked = assets
        .data_assets()
        .get::<AssetProbe>("stats/player.data")
        .unwrap();
    data.with(|file| {
        drop(assets);
        assert!(
            !linked.is_alive(),
            "an accessed data file must not keep the uniquely owned data cell alive"
        );
        let error = file.with::<AssetProbe, _>(|asset| asset.value).unwrap_err();
        assert!(error.to_string().contains("expired"));
    })
    .unwrap();
    assert!(!data.is_alive());
}

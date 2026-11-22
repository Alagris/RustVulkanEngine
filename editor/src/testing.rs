//! Test support for driving the editor against arbitrary asset directories.
//!
//! The framework lets unit and integration tests point the [`AssetDatabase`] and
//! [`Project`] loaders at any folder on disk:
//!
//! * [`AssetDir::open`] uses an existing folder as the asset root.
//! * [`AssetDir::fixture`] uses a checked-in folder from `editor/tests/resources`.
//! * [`AssetDir::temp`] creates a unique scratch folder that is deleted on drop.
//! * [`AssetDir::temp_copy_of_fixture`] clones a fixture so tests can mutate it.
//!
//! [`ProjectDir`] offers the same entry points for whole projects, which consist
//! of a `settings.yaml` next to an `assets` directory.

use crate::asset_database::{AssetDatabase, file_nonce};
use crate::project::Project;
use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};

/// Returns the root folder holding the checked-in test resources.
pub fn resources_root() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("resources");
    assert!(
        root.is_dir(),
        "test resources directory is missing: {}",
        root.display()
    );
    root
}

/// Renders the full `anyhow` error chain so tests can match any nested message.
pub fn error_chain(error: &anyhow::Error) -> String {
    format!("{error:#}")
}

/// Asserts that `error` mentions `needle` anywhere in its context chain.
pub fn assert_error_contains(error: &anyhow::Error, needle: &str) {
    let chain = error_chain(error);
    assert!(
        chain.contains(needle),
        "expected error to mention '{needle}' but it was:\n{chain}"
    );
}

/// Scans an arbitrary path expecting a failure and returns the error chain.
pub fn scan_error_at(root: impl AsRef<Path>) -> String {
    match AssetDatabase::scan(root.as_ref()) {
        Ok(_) => panic!(
            "expected scanning {} to fail, but it succeeded",
            root.as_ref().display()
        ),
        Err(error) => error_chain(&error),
    }
}

/// Loads a project from an arbitrary settings path expecting a failure.
pub fn project_error_at(settings_path: impl AsRef<Path>) -> String {
    match Project::load(settings_path.as_ref()) {
        Ok(_) => panic!(
            "expected loading {} to fail, but it succeeded",
            settings_path.as_ref().display()
        ),
        Err(error) => error_chain(&error),
    }
}

/// An asset directory the editor can scan during tests.
///
/// Directories created through [`AssetDir::temp`] or
/// [`AssetDir::temp_copy_of_fixture`] are owned and removed when dropped;
/// directories referenced through [`AssetDir::open`] or [`AssetDir::fixture`]
/// are borrowed and left untouched.
pub struct AssetDir {
    root: PathBuf,
    owned: bool,
}

impl AssetDir {
    /// Uses any existing folder as the asset directory.
    pub fn open(root: impl AsRef<Path>) -> Self {
        let root = root.as_ref().to_path_buf();
        assert!(
            root.is_dir(),
            "asset directory does not exist: {}",
            root.display()
        );
        Self { root, owned: false }
    }

    /// Uses a checked-in fixture from `editor/tests/resources`.
    pub fn fixture(name: &str) -> Self {
        assert!(!name.trim().is_empty(), "fixture names must not be empty");
        let root = resources_root().join(name);
        assert!(root.is_dir(), "unknown fixture folder: {}", root.display());
        Self { root, owned: false }
    }

    /// Creates an empty unique scratch directory that is deleted on drop.
    pub fn temp(label: &str) -> Self {
        assert!(!label.trim().is_empty(), "temp labels must not be empty");
        let root = std::env::temp_dir().join(format!(
            "editor-testing-{label}-{}-{}",
            std::process::id(),
            file_nonce()
        ));
        fs::create_dir_all(&root).expect("failed to create scratch asset directory");
        Self { root, owned: true }
    }

    /// Clones a checked-in fixture into a scratch directory so tests can mutate it.
    pub fn temp_copy_of_fixture(name: &str) -> Self {
        let source = Self::fixture(name);
        let copy = Self::temp(&name.replace(['/', '\\'], "-"));
        copy_dir(&source.root, &copy.root).expect("failed to copy fixture");
        copy
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn path(&self, relative: impl AsRef<Path>) -> PathBuf {
        self.root.join(relative)
    }

    /// Writes a file relative to the asset root, creating parent directories.
    pub fn write(&self, relative: impl AsRef<Path>, contents: &str) -> PathBuf {
        let path = self.root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("failed to create asset parent directory");
        }
        fs::write(&path, contents).expect("failed to write asset file");
        assert!(path.is_file(), "written asset must exist");
        path
    }

    /// Deletes a file relative to the asset root.
    pub fn remove(&self, relative: impl AsRef<Path>) {
        let path = self.root.join(relative);
        fs::remove_file(&path).expect("failed to remove asset file");
    }

    /// Scans this directory as an asset database.
    pub fn scan(&self) -> Result<AssetDatabase> {
        AssetDatabase::scan(&self.root)
    }

    /// Scans this directory, panicking with the error chain if the scan fails.
    pub fn scan_or_panic(&self) -> AssetDatabase {
        match self.scan() {
            Ok(database) => database,
            Err(error) => panic!(
                "expected {} to scan cleanly: {}",
                self.root.display(),
                error_chain(&error)
            ),
        }
    }

    /// Scans this directory expecting a failure and returns the error chain.
    pub fn scan_error(&self) -> String {
        match self.scan() {
            Ok(_) => panic!(
                "expected scanning {} to fail, but it succeeded",
                self.root.display()
            ),
            Err(error) => error_chain(&error),
        }
    }
}

impl Drop for AssetDir {
    fn drop(&mut self) {
        if self.owned {
            // Best-effort cleanup; leaked scratch folders only waste temp space.
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

/// A project directory holding `settings.yaml` and an `assets` folder.
pub struct ProjectDir {
    dir: AssetDir,
}

impl ProjectDir {
    /// Uses any existing folder as the project root.
    pub fn open(root: impl AsRef<Path>) -> Self {
        Self {
            dir: AssetDir::open(root),
        }
    }

    /// Uses a checked-in project fixture from `editor/tests/resources`.
    pub fn fixture(name: &str) -> Self {
        Self {
            dir: AssetDir::fixture(name),
        }
    }

    /// Creates an empty scratch project with an `assets` subdirectory.
    pub fn temp(label: &str) -> Self {
        let dir = AssetDir::temp(label);
        fs::create_dir_all(dir.path("assets")).expect("failed to create assets directory");
        Self { dir }
    }

    /// Clones a checked-in project fixture into a scratch directory.
    pub fn temp_copy_of_fixture(name: &str) -> Self {
        Self {
            dir: AssetDir::temp_copy_of_fixture(name),
        }
    }

    pub fn root(&self) -> &Path {
        self.dir.root()
    }

    pub fn settings_path(&self) -> PathBuf {
        self.dir.path("settings.yaml")
    }

    /// Writes the project `settings.yaml`.
    pub fn write_settings(&self, contents: &str) -> PathBuf {
        self.dir.write("settings.yaml", contents)
    }

    /// Writes a file relative to the project's `assets` directory.
    pub fn write_asset(&self, relative: impl AsRef<Path>, contents: &str) -> PathBuf {
        self.dir.write(Path::new("assets").join(relative), contents)
    }

    /// Loads the project from its settings file.
    pub fn load(&self) -> Result<Project> {
        Project::load(self.settings_path())
    }

    /// Loads the project, panicking with the error chain if loading fails.
    pub fn load_or_panic(&self) -> Project {
        match self.load() {
            Ok(project) => project,
            Err(error) => panic!(
                "expected project {} to load cleanly: {}",
                self.root().display(),
                error_chain(&error)
            ),
        }
    }

    /// Loads the project expecting a failure and returns the error chain.
    pub fn load_error(&self) -> String {
        match self.load() {
            Ok(_) => panic!(
                "expected loading project {} to fail, but it succeeded",
                self.root().display()
            ),
            Err(error) => error_chain(&error),
        }
    }
}

fn copy_dir(source: &Path, destination: &Path) -> Result<()> {
    assert!(source.is_dir(), "copy source must be a directory");
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let target = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temp_asset_dirs_are_unique_writable_and_cleaned_up() {
        let first = AssetDir::temp("framework");
        let second = AssetDir::temp("framework");
        assert_ne!(first.root(), second.root());
        let file = first.write("nested/folder/test.txt", "hello");
        assert_eq!(fs::read_to_string(&file).unwrap(), "hello");
        let root = first.root().to_path_buf();
        drop(first);
        assert!(!root.exists(), "owned scratch directories must be removed");
    }

    #[test]
    fn open_points_at_any_existing_folder() {
        let scratch = AssetDir::temp("open");
        scratch.write("standalone.scene", "root:\n  name: Standalone\n");
        let opened = AssetDir::open(scratch.root());
        let database = opened.scan_or_panic();
        assert_eq!(database.assets().count(), 1);
        let root = scratch.root().to_path_buf();
        drop(opened);
        assert!(root.exists(), "borrowed directories must not be deleted");
    }

    #[test]
    fn scan_error_reports_the_full_context_chain() {
        let scratch = AssetDir::temp("error-chain");
        scratch.write("broken.scene", "root: [not, a, mapping]\n");
        let error = scratch.scan_error();
        assert!(error.contains("broken.scene"), "unexpected error: {error}");
    }
}

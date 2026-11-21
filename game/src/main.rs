fn main() -> anyhow::Result<()> {
    let settings = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("settings.toml");
    editor::cli::run_cli(settings)
}

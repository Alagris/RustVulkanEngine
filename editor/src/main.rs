use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    editor::cli::run_cli(PathBuf::from("settings.yaml"))
}

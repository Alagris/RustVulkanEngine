use crate::{AssetDatabase, Project};
use anyhow::{Context, Result, anyhow};
use engine::render_loop::{RenderLoop, RenderLoopOptions};
use engine::{ecs::ComponentRegistry, settings::RuntimeSettings};
use std::path::{Path, PathBuf};

pub fn run_cli(default_settings: impl AsRef<Path>) -> Result<()> {
    if std::env::var_os("RUST_LOG").is_none() {
        unsafe {
            std::env::set_var("RUST_LOG", "info");
        }
    }
    pretty_env_logger::init();
    let mut args = std::env::args_os().skip(1);
    let command = args.next();
    match command.as_deref().and_then(|value| value.to_str()) {
        Some("index") => {
            let root = PathBuf::from(
                args.next()
                    .ok_or_else(|| anyhow!("usage: editor index <asset-root>"))?,
            );
            if args.next().is_some() {
                return Err(anyhow!("index accepts only an asset root"));
            }
            let database = AssetDatabase::scan(root)?;
            for (reference, path) in database.assets() {
                println!(
                    "{:?}:{} -> {}",
                    reference.asset_type,
                    reference.name,
                    path.display()
                );
            }
            Ok(())
        }
        Some("package") => {
            let settings_path =
                PathBuf::from(args.next().ok_or_else(|| {
                    anyhow!("usage: editor package <settings.yaml> <output-dir>")
                })?);
            let output = PathBuf::from(
                args.next()
                    .ok_or_else(|| anyhow!("package requires an output directory"))?,
            );
            if args.next().is_some() {
                return Err(anyhow!("too many package arguments"));
            }
            let project = Project::load(settings_path)?;
            let packaged_settings = project.package(&output)?;
            println!("Packaged runtime settings: {}", packaged_settings.display());
            Ok(())
        }
        Some("run") | None => run_game(default_settings.as_ref(), args),
        Some("--help") | Some("-h") | Some("help") => {
            print_usage();
            Ok(())
        }
        Some(option) if option.starts_with("--") => run_game(
            default_settings.as_ref(),
            std::iter::once(std::ffi::OsString::from(option)).chain(args),
        ),
        Some(other) => Err(anyhow!("unknown editor command '{other}'")),
    }
}

fn run_game(
    default_settings: &Path,
    mut args: impl Iterator<Item = std::ffi::OsString>,
) -> Result<()> {
    let mut settings_path = default_settings.to_path_buf();
    let mut release_mode = false;
    let mut compile_only = false;
    let mut frame_limit = None;
    while let Some(argument) = args.next() {
        match argument.to_str() {
            Some("--settings") => {
                settings_path = args
                    .next()
                    .map(PathBuf::from)
                    .ok_or_else(|| anyhow!("--settings requires a YAML file path"))?;
            }
            Some("--release") => release_mode = true,
            Some("--compile-shaders") => compile_only = true,
            Some("--frames") => {
                let frames = args
                    .next()
                    .ok_or_else(|| anyhow!("--frames requires a positive frame count"))?
                    .to_string_lossy()
                    .parse::<u32>()
                    .context("--frames requires a positive integer")?;
                if frames == 0 {
                    return Err(anyhow!("--frames requires a positive frame count"));
                }
                frame_limit = Some(frames);
            }
            Some("--help") | Some("-h") => {
                print_usage();
                return Ok(());
            }
            _ => {
                return Err(anyhow!(
                    "unknown run option: {}",
                    argument.to_string_lossy()
                ));
            }
        }
    }

    let runtime_settings = if release_mode {
        RuntimeSettings::load(&settings_path)?
    } else {
        Project::load(&settings_path)?.prepare_development()?
    };
    RenderLoop::run(
        runtime_settings,
        RenderLoopOptions {
            frame_limit,
            compile_only,
            title: "Rust Vulkan Game Engine".to_owned(),
        },
        ComponentRegistry::new(),
    )
}

fn print_usage() {
    println!(
        "Usage:\n  editor [run] [--settings <settings.yaml>] [--release] [--compile-shaders] [--frames <count>]\n  editor index <asset-root>\n  editor package <settings.yaml> <output-dir>"
    );
}

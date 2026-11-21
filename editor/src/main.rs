use anyhow::{Context, Result, anyhow};
use editor::{AssetDatabase, AssetRef, AssetType};
use std::path::PathBuf;

fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let command = args.next().ok_or_else(|| {
        anyhow!("usage: editor <index|package> <asset-root> [pipeline-name] [output]")
    })?;
    let root = PathBuf::from(
        args.next()
            .ok_or_else(|| anyhow!("an asset root directory is required"))?,
    );
    let database = AssetDatabase::scan(&root)?;
    match command.to_string_lossy().as_ref() {
        "index" => {
            if args.next().is_some() {
                return Err(anyhow!("index accepts only an asset root"));
            }
            for (reference, path) in database.assets() {
                println!(
                    "{:?}:{} -> {}",
                    reference.asset_type,
                    reference.name,
                    path.display()
                );
            }
        }
        "package" => {
            let pipeline_name = args
                .next()
                .ok_or_else(|| anyhow!("package requires a pipeline asset name"))?
                .to_string_lossy()
                .into_owned();
            let output = PathBuf::from(
                args.next()
                    .ok_or_else(|| anyhow!("package requires an output directory"))?,
            );
            if args.next().is_some() {
                return Err(anyhow!("too many package arguments"));
            }
            let pipeline = database.resolve(&AssetRef {
                name: pipeline_name,
                asset_type: AssetType::Pipeline,
            })?;
            let packaged = database
                .package(pipeline, &output)
                .with_context(|| format!("Failed to package assets to {}", output.display()))?;
            println!("Packaged pipeline: {}", packaged.display());
        }
        _ => {
            return Err(anyhow!(
                "unknown editor command: {}",
                command.to_string_lossy()
            ));
        }
    }
    Ok(())
}

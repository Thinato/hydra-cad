//! Admin tool: run migrations, import catalogue files, attach datasheets and 3D models.

use std::process::{Command, ExitCode};

use hydra_core::{Catalogue, CatalogueRef};
use hydra_db::{
    MIGRATOR, ModelOwner, asset_bytes, attach_datasheet, attach_model, check_schema, connect,
    import_catalogue, load_catalogue, sha256_hex,
};

const USAGE: &str = "usage: hydra-admin <command>   (reads DATABASE_URL)
  migrate                        apply migrations and check the schema
  import <catalogue.json>        import catalogue rows
  attach <key@version> <file.pdf> attach a PDF datasheet to a valve type
  model valve|block <key@version> <file.glb> [--source <file.step>]
  model valve|block <key@version> <file.step> [--linear <mm>]
                                 attach a 3D model; a STEP file is converted with
                                 $HYDRA_STEP2GLTF (default step2gltf) and kept as source
  models                         check every stored 3D model
  check                          check the schema version";

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args.iter().map(String::as_str).collect::<Vec<_>>()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("hydra-admin: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn run(args: &[&str]) -> Result<(), Box<dyn std::error::Error>> {
    if !matches!(
        args,
        ["migrate" | "check" | "models"]
            | ["import", _]
            | ["attach", _, _]
            | ["model", "valve" | "block", _, _, ..]
    ) {
        return Err(USAGE.into());
    }
    let url = std::env::var("DATABASE_URL").map_err(|_| "DATABASE_URL is not set")?;
    let pool = connect(&url).await?;
    match args {
        ["migrate"] => {
            MIGRATOR.run(&pool).await?;
            check_schema(&pool).await?;
            let version = MIGRATOR.iter().map(|m| m.version).max().unwrap_or(0);
            println!("schema migrated to version {version}");
        }
        ["check"] => {
            check_schema(&pool).await?;
            println!("OK");
        }
        ["import", path] => {
            let catalogue: Catalogue = serde_json::from_str(&std::fs::read_to_string(path)?)?;
            let r = import_catalogue(&pool, &catalogue).await?;
            println!(
                "added {} manufacturers, {} symbol defs, {} valve types, {} parts, {} blocks; \
                 {} unchanged",
                r.manufacturers_added,
                r.symbol_defs_added,
                r.valve_types_added,
                r.parts_added,
                r.blocks_added,
                r.unchanged
            );
        }
        ["attach", reference, path] => {
            let reference = CatalogueRef::try_from(reference.to_string())?;
            let sha = attach_datasheet(&pool, &reference, &std::fs::read(path)?).await?;
            println!("{sha}");
        }
        ["model", kind, reference, path, flags @ ..] => {
            let (mut source, mut linear) = (None, None);
            for pair in flags.chunks(2) {
                match pair {
                    ["--source", f] => source = Some(*f),
                    ["--linear", mm] => linear = Some(*mm),
                    _ => return Err(USAGE.into()),
                }
            }
            let reference = CatalogueRef::try_from(reference.to_string())?;
            let owner = match *kind {
                "valve" => ModelOwner::ValveType(reference),
                _ => ModelOwner::Block(reference),
            };
            let lower = path.to_ascii_lowercase();
            let sha = if lower.ends_with(".step") || lower.ends_with(".stp") {
                if source.is_some() {
                    return Err(
                        "--source is only for a glb file; a STEP file is its own source".into(),
                    );
                }
                let step = std::fs::read(path)?;
                let glb = step_to_glb(path, linear)?;
                attach_model(&pool, &owner, &glb, Some(&step)).await?
            } else {
                if linear.is_some() {
                    return Err("--linear is only for a STEP file".into());
                }
                let step = source.map(std::fs::read).transpose()?;
                attach_model(&pool, &owner, &std::fs::read(path)?, step.as_deref()).await?
            };
            println!("{sha}");
        }
        ["models"] => {
            let loaded = load_catalogue(&pool).await?;
            let mut models: Vec<_> = (loaded.valve_models.iter().map(|(r, s)| ("valve", r, s)))
                .chain(loaded.block_models.iter().map(|(r, s)| ("block", r, s)))
                .collect();
            models.sort();
            let mut failed = 0;
            for (kind, r, sha) in &models {
                let checked = match asset_bytes(&pool, sha).await? {
                    None => Err(format!("asset {sha} is missing")),
                    Some(bytes) if sha256_hex(&bytes) != **sha => {
                        Err(format!("asset {sha} does not match its hash"))
                    }
                    Some(bytes) => hydra_mesh::load_glb(&bytes),
                };
                match checked {
                    Ok(mesh) => {
                        let (lo, hi) = mesh.bounds();
                        println!(
                            "ok   {kind} {r}  {} triangles  {:.0}x{:.0}x{:.0} mm",
                            mesh.indices.len() / 3,
                            hi[0] - lo[0],
                            hi[1] - lo[1],
                            hi[2] - lo[2]
                        );
                    }
                    Err(e) => {
                        failed += 1;
                        println!("FAIL {kind} {r}  {e}");
                    }
                }
            }
            if failed > 0 {
                return Err(format!("{failed} of {} models failed", models.len()).into());
            }
        }
        _ => unreachable!(),
    }
    Ok(())
}

/// Convert a STEP file to glb bytes with the external converter.
fn step_to_glb(step: &str, linear: Option<&str>) -> Result<Vec<u8>, String> {
    let prog = std::env::var("HYDRA_STEP2GLTF").unwrap_or_else(|_| "step2gltf".into());
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let tmp = std::env::temp_dir().join(format!("hydra-{}-{nanos}.glb", std::process::id()));
    // Created exclusively, so a file or symlink planted at that name in a shared temp
    // directory is refused rather than written through.
    std::fs::File::create_new(&tmp)
        .map_err(|e| format!("can't create {} for the converter: {e}", tmp.display()))?;
    let mut cmd = Command::new(&prog);
    // The converter's summary goes to stderr, so stdout carries only the hash.
    cmd.arg(step).arg(&tmp).stdout(std::io::stderr());
    if let Some(mm) = linear {
        cmd.args(["--linear", mm]);
    }
    let result = match cmd.status() {
        Err(e) => Err(format!(
            "can't run the STEP converter {prog:?} ({e}); set HYDRA_STEP2GLTF to its path"
        )),
        Ok(status) if !status.success() => Err(format!(
            "the STEP converter {prog:?} failed ({status}); check the file, or set \
             HYDRA_STEP2GLTF to another converter"
        )),
        Ok(_) => std::fs::read(&tmp).map_err(|e| {
            format!("the STEP converter {prog:?} wrote no model ({e}); check HYDRA_STEP2GLTF")
        }),
    };
    let _ = std::fs::remove_file(&tmp);
    result
}

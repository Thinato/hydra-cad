//! Admin tool: run migrations, import catalogue files and attach datasheets.

use std::process::ExitCode;

use hydra_core::{Catalogue, CatalogueRef};
use hydra_db::{MIGRATOR, attach_datasheet, check_schema, connect, import_catalogue};

const USAGE: &str = "usage: hydra-admin <command>   (reads DATABASE_URL)
  migrate                        apply migrations and check the schema
  import <catalogue.json>        import catalogue rows
  attach <key@version> <file.pdf> attach a PDF datasheet to a valve type
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
        ["migrate" | "check"] | ["import", _] | ["attach", _, _]
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
                "added {} manufacturers, {} symbol defs, {} valve types, {} parts; {} unchanged",
                r.manufacturers_added,
                r.symbol_defs_added,
                r.valve_types_added,
                r.parts_added,
                r.unchanged
            );
        }
        ["attach", reference, path] => {
            let reference = CatalogueRef::try_from(reference.to_string())?;
            let sha = attach_datasheet(&pool, &reference, &std::fs::read(path)?).await?;
            println!("{sha}");
        }
        _ => unreachable!(),
    }
    Ok(())
}

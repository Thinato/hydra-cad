//! `hydra-mcp`: the MCP server over stdio. stdout carries the protocol; logs go to stderr.

use std::process::ExitCode;

use hydra_mcp::HydraMcp;
use rmcp::ServiceExt;

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("hydra-mcp: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), String> {
    let url = std::env::var("HYDRA_DATABASE_URL").map_err(|_| {
        "HYDRA_DATABASE_URL is not set; set it to the Hydra database, \
         e.g. postgres://user:password@server:5433/hydra"
            .to_string()
    })?;
    let pool = hydra_db::connect(&url)
        .await
        .map_err(|e| format!("cannot connect to the Hydra database: {e}"))?;
    hydra_db::check_schema(&pool)
        .await
        .map_err(|e| format!("the Hydra database can't be used: {e}"))?;
    let service = HydraMcp::new(pool)
        .serve(rmcp::transport::stdio())
        .await
        .map_err(|e| format!("MCP session failed to start: {e}"))?;
    service
        .waiting()
        .await
        .map_err(|e| format!("MCP session ended with an error: {e}"))?;
    Ok(())
}

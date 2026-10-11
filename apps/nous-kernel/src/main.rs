// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use clap::Parser;
use nous_core::{Error, Result};
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio_stream::wrappers::TcpListenerStream;
mod bootstrap;

const BOOTSTRAP_CREDENTIAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const MAX_BOOTSTRAP_CREDENTIAL_LINE_BYTES: u64 = 130;

#[derive(Parser)]
struct Cli {
    #[arg(long)]
    config: Option<PathBuf>,
    #[arg(long)]
    check_configuration: Option<PathBuf>,
}
#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    if let Err(error) = run().await {
        tracing::error!(%error,"Kernel failed");
        std::process::exit(1);
    }
}
#[expect(
    clippy::print_stdout,
    reason = "kernel bootstrap emits the machine-readable endpoint to its parent process"
)]
async fn run() -> Result<()> {
    let cli = Cli::parse();
    if let Some(path) = &cli.check_configuration {
        let bundle = bootstrap::read_bundle(path).await?;
        let catalog = nous_kernel::configuration_catalog(bundle.core_descriptors)?;
        catalog.deployment_values(&bundle.deployment_document)?;
        println!(
            "{}",
            serde_json::json!({"valid":true,"catalog_digest":catalog.catalog_digest()})
        );
        return Ok(());
    }
    let config = cli
        .config
        .ok_or_else(|| Error::Invalid("--config is required".into()))?;
    let mut input = BufReader::new(tokio::io::stdin());
    let mut token = String::new();
    tokio::time::timeout(
        BOOTSTRAP_CREDENTIAL_TIMEOUT,
        (&mut input)
            .take(MAX_BOOTSTRAP_CREDENTIAL_LINE_BYTES)
            .read_line(&mut token),
    )
    .await
    .map_err(|_| Error::Invalid("Kernel bootstrap timed out".into()))?
    .map_err(|e| Error::Invalid(e.to_string()))?;
    let token = token.trim().to_owned();
    if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::Invalid("invalid Kernel bootstrap credential".into()));
    }
    let (runtime, managed) = bootstrap::open(&config).await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| Error::Infrastructure(e.to_string()))?;
    let endpoint = listener
        .local_addr()
        .map_err(|e| Error::Infrastructure(e.to_string()))?;
    let server = nous_kernel::transport::router(runtime.clone(), token)
        .await
        .serve_with_incoming(TcpListenerStream::new(listener));
    println!(
        "{}",
        serde_json::json!({"endpoint":format!("http://{endpoint}")})
    );
    let result = tokio::select! {
        result=server=>result.map_err(|e|Error::Infrastructure(e.to_string())),
        _=async {let mut byte=[0u8;1];loop{match input.read(&mut byte).await{Ok(0)|Err(_)=>break,Ok(_)=>{}}}}=>Ok(()),
        _=tokio::signal::ctrl_c()=>Ok(()),
    };
    runtime.store.close().await;
    bootstrap::stop_managed(managed).await?;
    result
}

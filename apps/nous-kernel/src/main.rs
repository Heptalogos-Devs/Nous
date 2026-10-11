// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use clap::Parser;
use nous_core::{Error, Result};
use sqlx::Connection;
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
    /// Convert legacy vector ownership on an operator-provided offline copy.
    #[arg(long)]
    upgrade_embedding_ownership_copy: bool,
    /// PostgreSQL URL for the copied/offline target database only.
    #[arg(long)]
    copied_offline_database_url: Option<String>,
    /// Existing object-store directory from the same offline data copy.
    #[arg(long)]
    copied_object_root: Option<PathBuf>,
    /// Original source database name. The target database must have a different name.
    #[arg(long)]
    source_database_name: Option<String>,
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
        if cli.upgrade_embedding_ownership_copy
            || cli.copied_offline_database_url.is_some()
            || cli.copied_object_root.is_some()
            || cli.source_database_name.is_some()
            || std::env::var_os("NOUS_WAVE_COPIED_OFFLINE_DATABASE_URL").is_some()
        {
            return Err(cli_error(
                "configuration check and legacy embedding upgrade are separate commands",
            ));
        }
        let bundle = bootstrap::read_bundle(path).await?;
        let catalog = nous_kernel::configuration_catalog(bundle.core_descriptors)?;
        catalog.deployment_values(&bundle.deployment_document)?;
        println!(
            "{}",
            serde_json::json!({"valid":true,"catalog_digest":catalog.catalog_digest()})
        );
        return Ok(());
    }
    if cli.upgrade_embedding_ownership_copy {
        let config = cli
            .config
            .as_deref()
            .ok_or_else(|| cli_error("--config is required for the copied data"))?;
        let copied_database_url = cli
            .copied_offline_database_url
            .or_else(|| std::env::var("NOUS_WAVE_COPIED_OFFLINE_DATABASE_URL").ok())
            .ok_or_else(|| {
                cli_error("--copied-offline-database-url or NOUS_WAVE_COPIED_OFFLINE_DATABASE_URL is required")
            })?;
        let copied_object_root = cli
            .copied_object_root
            .as_deref()
            .ok_or_else(|| cli_error("--copied-object-root is required"))?;
        let source_database_name = cli
            .source_database_name
            .as_deref()
            .filter(|name| !name.trim().is_empty())
            .ok_or_else(|| cli_error("--source-database-name is required"))?;
        return upgrade_embedding_ownership_copy(
            config,
            copied_database_url,
            copied_object_root,
            source_database_name,
        )
        .await;
    }
    if cli.copied_offline_database_url.is_some()
        || cli.copied_object_root.is_some()
        || cli.source_database_name.is_some()
        || std::env::var_os("NOUS_WAVE_COPIED_OFFLINE_DATABASE_URL").is_some()
    {
        return Err(cli_error(
            "copied-data flags require --upgrade-embedding-ownership-copy",
        ));
    }
    let config = cli
        .config
        .ok_or_else(|| cli_error("--config is required"))?;
    let mut input = BufReader::new(tokio::io::stdin());
    let mut token = String::new();
    tokio::time::timeout(
        BOOTSTRAP_CREDENTIAL_TIMEOUT,
        (&mut input)
            .take(MAX_BOOTSTRAP_CREDENTIAL_LINE_BYTES)
            .read_line(&mut token),
    )
    .await
    .map_err(|error| {
        Error::new(
            nous_core::ErrorOwner::Kernel,
            nous_core::ErrorCode::InvalidRequest,
            "Kernel bootstrap timed out",
        )
        .with_source(error)
    })?
    .map_err(|e| {
        Error::new(
            nous_core::ErrorOwner::Kernel,
            nous_core::ErrorCode::InvalidRequest,
            "Invalid request",
        )
        .with_source(e)
    })?;
    let token = token.trim().to_owned();
    if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::new(
            nous_core::ErrorOwner::Kernel,
            nous_core::ErrorCode::InvalidRequest,
            "invalid Kernel bootstrap credential",
        ));
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|error| {
            Error::new(
                nous_core::ErrorOwner::Kernel,
                nous_core::ErrorCode::Unavailable,
                "Kernel listener could not start",
            )
            .with_source(error)
        })?;
    let endpoint = listener.local_addr().map_err(|error| {
        Error::new(
            nous_core::ErrorOwner::Kernel,
            nous_core::ErrorCode::InternalFailure,
            "Kernel endpoint could not be read",
        )
        .with_source(error)
    })?;
    let (runtime, managed) = bootstrap::open(&config).await?;
    println!(
        "{}",
        serde_json::json!({"endpoint":format!("http://{endpoint}")})
    );
    let result = {
        let server = nous_kernel::transport::router(runtime.clone(), token)
            .await
            .serve_with_incoming(TcpListenerStream::new(listener));
        tokio::pin!(server);
        let mut lock_health = tokio::time::interval(std::time::Duration::from_secs(1));
        loop {
            tokio::select! {
                result=&mut server=>break result.map_err(|error| {
                    Error::new(
                        nous_core::ErrorOwner::Kernel,
                        nous_core::ErrorCode::InternalFailure,
                        "Kernel server stopped unexpectedly",
                    )
                    .with_source(error)
                }),
                _=async {let mut byte=[0u8;1];loop{match input.read(&mut byte).await{Ok(0)|Err(_)=>break,Ok(_)=>{}}}}=>break Ok(()),
                _=tokio::signal::ctrl_c()=>break Ok(()),
                _=lock_health.tick()=> {
                    if let Err(error)=runtime.store.check_instance_lock().await {
                        break Err(error);
                    }
                }
            }
        }
    };
    runtime.store.close().await;
    bootstrap::stop_managed(managed).await?;
    result
}

#[expect(
    clippy::print_stdout,
    reason = "offline conversion emits a concise operator report"
)]
async fn upgrade_embedding_ownership_copy(
    config: &std::path::Path,
    copied_database_url: String,
    copied_object_root: &std::path::Path,
    source_database_name: &str,
) -> Result<()> {
    let mut connection = sqlx::PgConnection::connect(&copied_database_url)
        .await
        .map_err(|error| {
            Error::new(
                nous_core::ErrorOwner::Persistence,
                nous_core::ErrorCode::Unavailable,
                "Copied target database is unavailable",
            )
            .with_source(error)
        })?;
    let target_database_name: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&mut connection)
        .await
        .map_err(|error| {
            Error::new(
                nous_core::ErrorOwner::Persistence,
                nous_core::ErrorCode::Unavailable,
                "Copied target database is unavailable",
            )
            .with_source(error)
        })?;
    connection.close().await.map_err(|error| {
        Error::new(
            nous_core::ErrorOwner::Persistence,
            nous_core::ErrorCode::InternalFailure,
            "Copied target database session could not close",
        )
        .with_source(error)
    })?;
    if target_database_name == source_database_name {
        return Err(cli_error(
            "refusing to upgrade the source database; destination name must differ",
        ));
    }

    let serving_root = std::env::temp_dir().join(format!(
        "nous-wave-embedding-upgrade-{}",
        uuid::Uuid::now_v7()
    ));
    std::fs::create_dir(&serving_root).map_err(|error| {
        Error::new(
            nous_core::ErrorOwner::Instance,
            nous_core::ErrorCode::InternalFailure,
            "Temporary Serving directory could not be created",
        )
        .with_source(error)
    })?;
    let result = async {
        let runtime = bootstrap::open_embedding_upgrade_copy(
            config,
            copied_database_url,
            copied_object_root,
            &serving_root,
        )
        .await?;
        let actual_database_name = runtime.store.database_name().await?;
        if actual_database_name == source_database_name {
            return Err(cli_error(
                "refusing to upgrade the source database; destination name must differ",
            ));
        }
        let report = runtime.serving.migrate_legacy_embedding_uses().await?;
        runtime.store.close().await;
        Ok::<_, Error>((actual_database_name, report))
    }
    .await;
    let cleanup = std::fs::remove_dir_all(&serving_root).map_err(|error| {
        Error::new(
            nous_core::ErrorOwner::Instance,
            nous_core::ErrorCode::InternalFailure,
            "Temporary Serving directory could not be removed",
        )
        .with_source(error)
    });
    let (actual_database_name, report) = result?;
    cleanup?;
    println!(
        "{}",
        serde_json::json!({
            "converted": true,
            "copied_database": actual_database_name,
            "legacy_rows_examined": report.legacy_rows_examined,
            "references_examined": report.references_examined,
            "recovered_rows": report.recovered_rows,
            "backed_up_rows": report.backed_up_rows,
            "backup_table": "legacy_embedding_materials_backup",
            "backup_scope": "outside active cache and semantic-owner purge; delete through the independent backup lifecycle"
        })
    );
    Ok(())
}

fn cli_error(message: impl Into<String>) -> Error {
    Error::new(
        nous_core::ErrorOwner::Cli,
        nous_core::ErrorCode::InvalidRequest,
        message,
    )
}

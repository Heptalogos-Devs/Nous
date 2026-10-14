//! Explicit developer runtime preparation.
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use postgresql_embedded::{PostgreSQL, SettingsBuilder, VersionReq};
use std::{path::PathBuf, process::Command, time::Duration};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let installation = std::env::var_os("NOUS_WAVE_POSTGRES_RUNTIME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../data/runtime/installed/postgresql")
        });
    let executable = installation.join(if cfg!(windows) {
        "bin/postgres.exe"
    } else {
        "bin/postgres"
    });
    if executable.is_file() {
        let version = Command::new(&executable).arg("--version").output()?;
        if !version.status.success()
            || String::from_utf8(version.stdout)?.split_whitespace().nth(2) != Some("18.6")
        {
            return Err("Developer PostgreSQL must be version18.6; replace it explicitly".into());
        }
    } else {
        if installation.exists() {
            return Err("Partial development runtime installation; resolve it explicitly".into());
        }
        let parent = installation.parent().ok_or("Runtime parent required")?;
        std::fs::create_dir_all(parent)?;
        let scratch = parent.join(format!("prepare-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir(&scratch)?;
        let result: Result<(), Box<dyn std::error::Error>> = async {
            let settings = SettingsBuilder::new()
                .version(VersionReq::parse("=18.6.0")?)
                .installation_dir(scratch.join("install"))
                .data_dir(scratch.join("cluster"))
                .password_file(scratch.join("postgres.pgpass"))
                .password(uuid::Uuid::now_v7().to_string())
                .timeout(Some(Duration::from_secs(60)))
                .temporary(false)
                .build();
            let mut postgres = PostgreSQL::new(settings);
            postgres.setup().await?;
            let installed = postgres.settings().installation_dir.clone();
            drop(postgres);
            std::fs::rename(installed, &installation)?;
            Ok(())
        }
        .await;
        let scratch = scratch.canonicalize()?;
        if !scratch.starts_with(parent.canonicalize()?) {
            return Err("Unexpected runtime preparation path".into());
        }
        let cleanup = std::fs::remove_dir_all(scratch);
        result?;
        cleanup?;
    }
    println!("Developer PostgreSQL18.6 prepared; ordinary serve remains acquisition-free");
    Ok(())
}

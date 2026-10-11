// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use nous_core::{Error, Result};
use nous_kernel::{NousRuntime, RuntimeOptions};
use postgresql_embedded::{PostgreSQL, SettingsBuilder, VersionReq};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    bootstrap: BootstrapConfig,
    configuration_bundle: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct BootstrapConfig {
    database: DatabaseConfig,
    object_store: ObjectStoreConfig,
    serving: ServingBootstrapConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ServingBootstrapConfig {
    #[serde(default = "default_serving_root")]
    root: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct DatabaseConfig {
    mode: String,
    #[serde(default)]
    url: String,
    max_connections: u32,
    acquire_timeout_ms: u64,
    name: String,
    install_dir: String,
    data_dir: String,
    instance_dir: String,
    secret_dir: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ObjectStoreConfig {
    #[serde(default = "default_backend")]
    backend: String,
    #[serde(default = "default_object_root")]
    root: String,
}

fn default_backend() -> String {
    "fs".into()
}
fn default_object_root() -> String {
    "./data/objects".into()
}
fn default_serving_root() -> String {
    "./data/serving".into()
}

pub async fn open(path: &Path) -> Result<(NousRuntime, Option<PostgreSQL>)> {
    let text = tokio::fs::read_to_string(path)
        .await
        .map_err(|e| Error::Invalid(e.to_string()))?;
    let config: Config = toml::from_str(&text).map_err(|e| Error::Invalid(e.to_string()))?;
    if config.bootstrap.object_store.backend != "fs" {
        return Err(Error::Invalid("object store backend must be fs".into()));
    }
    let absolute = std::path::absolute(path).map_err(|e| Error::Invalid(e.to_string()))?;
    let root = absolute
        .parent()
        .ok_or_else(|| Error::Invalid("config parent required".into()))?;
    let bundle = read_bundle(&resolve_path(root, &config.configuration_bundle)).await?;
    // Validate the complete catalog/document before acquiring database resources.
    nous_kernel::configuration_catalog(bundle.core_descriptors.clone())?
        .deployment_values(&bundle.deployment_document)?;
    let (postgres_url, managed) = open_database(root, &config.bootstrap.database).await?;
    let result = NousRuntime::open(RuntimeOptions {
        postgres_url,
        max_connections: config.bootstrap.database.max_connections,
        acquire_timeout_ms: config.bootstrap.database.acquire_timeout_ms,
        object_root: resolve_path(root, &config.bootstrap.object_store.root)
            .to_string_lossy()
            .into_owned(),
        serving_options: nous_retrieval::ServingOptions {
            root: resolve_path(root, &config.bootstrap.serving.root),
            lexical: true,
            dense: true,
            topology: false,
            memory_enabled: true,
        },
        embedding: None,
        stored_embedding: None,
        core_descriptors: bundle.core_descriptors,
        deployment_document: bundle.deployment_document,
    })
    .await;
    match result {
        Ok(runtime) => Ok((runtime, managed)),
        Err(e) => {
            stop_managed(managed).await?;
            Err(e)
        }
    }
}
async fn open_database(
    root: &Path,
    config: &DatabaseConfig,
) -> Result<(String, Option<PostgreSQL>)> {
    match config.mode.as_str() {
        "external" => {
            if config.url.trim().is_empty() {
                return Err(Error::Invalid(
                    "external database mode requires database.url".into(),
                ));
            }
            Ok((config.url.clone(), None))
        }
        "managed_private" => {
            let settings = private_database_settings(root, config).await?;
            let mut postgres = PostgreSQL::new(settings);
            // The verified existing installation and trust_installation_dir make
            // setup initialization-only: the library cannot take its install branch.
            postgres.setup().await.map_err(|error| {
                Error::Infrastructure(format!("managed PostgreSQL setup: {error}"))
            })?;
            postgres.start().await.map_err(|error| {
                Error::Infrastructure(format!("managed PostgreSQL start: {error}"))
            })?;
            let database = if config.name.trim().is_empty() {
                "nous_wave_20260908"
            } else {
                &config.name
            };
            if !postgres
                .database_exists(database)
                .await
                .map_err(|error| Error::Infrastructure(error.to_string()))?
            {
                postgres
                    .create_database(database)
                    .await
                    .map_err(|error| Error::Infrastructure(error.to_string()))?;
            }
            Ok((postgres.settings().url(database), Some(postgres)))
        }
        other => Err(Error::Invalid(format!("unsupported database.mode {other}"))),
    }
}

async fn private_database_settings(
    root: &Path,
    config: &DatabaseConfig,
) -> Result<postgresql_embedded::Settings> {
    let install_dir = resolve_path(root, &config.install_dir);
    let data_dir = resolve_path(root, &config.data_dir);
    for executable in ["postgres", "initdb", "pg_ctl", "pg_isready"] {
        let name = if cfg!(windows) {
            format!("{executable}.exe")
        } else {
            executable.into()
        };
        if !install_dir.join("bin").join(name).is_file() {
            return Err(Error::Unavailable(
                "PostgreSQL runtime pack is missing; run nous runtime install postgresql".into(),
            ));
        }
    }
    let instance_dir = resolve_path(root, &config.instance_dir);
    let secret_dir = resolve_path(root, &config.secret_dir);
    tokio::fs::create_dir_all(&instance_dir)
        .await
        .map_err(|error| Error::Infrastructure(error.to_string()))?;
    tokio::fs::create_dir_all(&secret_dir)
        .await
        .map_err(|error| Error::Infrastructure(error.to_string()))?;
    if let Some(parent) = data_dir.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|error| Error::Infrastructure(error.to_string()))?;
    }
    let password_file = secret_dir.join("postgres.password");
    let password = match tokio::fs::read_to_string(&password_file).await {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let value = postgresql_embedded::Settings::default().password;
            let mut options = tokio::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            options.mode(0o600);
            let mut file = options
                .open(&password_file)
                .await
                .map_err(|error| Error::Infrastructure(error.to_string()))?;
            tokio::io::AsyncWriteExt::write_all(&mut file, value.as_bytes())
                .await
                .map_err(|error| Error::Infrastructure(error.to_string()))?;
            value
        }
        Err(error) => return Err(Error::Infrastructure(error.to_string())),
    };
    if password.is_empty() {
        return Err(Error::Invalid(
            "Private PostgreSQL credential is empty".into(),
        ));
    }
    #[derive(Deserialize, Serialize)]
    #[serde(deny_unknown_fields)]
    struct DatabaseState {
        port: u16,
        major: u32,
    }
    let state_file = instance_dir.join("postgres.json");
    let state = match tokio::fs::read(&state_file).await {
        Ok(bytes) => serde_json::from_slice::<DatabaseState>(&bytes)
            .map_err(|_| Error::Invalid("Private PostgreSQL state is invalid".into()))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .map_err(|error| Error::Infrastructure(error.to_string()))?;
            let value = DatabaseState {
                port: listener
                    .local_addr()
                    .map_err(|error| Error::Infrastructure(error.to_string()))?
                    .port(),
                major: 18,
            };
            tokio::fs::write(
                &state_file,
                serde_json::to_vec(&value)
                    .map_err(|error| Error::Infrastructure(error.to_string()))?,
            )
            .await
            .map_err(|error| Error::Infrastructure(error.to_string()))?;
            value
        }
        Err(error) => return Err(Error::Infrastructure(error.to_string())),
    };
    if state.port == 0 || state.major != 18 {
        return Err(Error::FailedPrecondition(
            "Private PostgreSQL version/port requires explicit migration".into(),
        ));
    }
    match tokio::fs::read_to_string(data_dir.join("PG_VERSION")).await {
        Ok(version) if version.trim() != "18" => {
            return Err(Error::FailedPrecondition(
                "PostgreSQL cluster major differs from runtime pack".into(),
            ));
        }
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            return Err(Error::Infrastructure(error.to_string()));
        }
        _ => {}
    }
    let settings = SettingsBuilder::new()
        .version(VersionReq::parse("=18.6.0").map_err(|error| Error::Invalid(error.to_string()))?)
        .host("127.0.0.1")
        .port(state.port)
        .username("postgres")
        .password(password)
        .installation_dir(install_dir)
        .trust_installation_dir(true)
        .data_dir(data_dir)
        .password_file(password_file)
        .temporary(false)
        .build();
    Ok(settings)
}

pub async fn stop_managed(mut postgres: Option<PostgreSQL>) -> Result<()> {
    if let Some(postgres) = postgres.as_mut() {
        postgres
            .stop()
            .await
            .map_err(|error| Error::Infrastructure(error.to_string()))?;
    }
    Ok(())
}

fn resolve_path(root: &Path, value: &str) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        root.join(path)
    }
}

pub async fn read_bundle(path: &Path) -> Result<nous_configuration::ConfigurationBootstrapBundle> {
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|error| Error::Invalid(error.to_string()))?;
    let bundle: nous_configuration::ConfigurationBootstrapBundle =
        serde_json::from_slice(&bytes)
            .map_err(|_| Error::Invalid("invalid private configuration bundle".into()))?;
    Ok(bundle)
}

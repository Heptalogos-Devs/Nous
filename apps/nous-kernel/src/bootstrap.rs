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

struct ResolvedConfiguration {
    config: Config,
    root: PathBuf,
    bundle: nous_configuration::ConfigurationBootstrapBundle,
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
    let loaded = read_configuration(path).await?;
    let (postgres_url, managed) =
        open_database(&loaded.root, &loaded.config.bootstrap.database).await?;
    let result = NousRuntime::open(RuntimeOptions {
        postgres_url,
        max_connections: loaded.config.bootstrap.database.max_connections,
        acquire_timeout_ms: loaded.config.bootstrap.database.acquire_timeout_ms,
        object_root: resolve_path(&loaded.root, &loaded.config.bootstrap.object_store.root)
            .to_string_lossy()
            .into_owned(),
        serving_options: nous_retrieval::ServingOptions {
            root: resolve_path(&loaded.root, &loaded.config.bootstrap.serving.root),
            lexical: true,
            dense: true,
            topology: false,
            memory_enabled: true,
        },
        embedding: None,
        stored_embedding: None,
        core_descriptors: loaded.bundle.core_descriptors,
        deployment_document: loaded.bundle.deployment_document,
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

async fn read_configuration(path: &Path) -> Result<ResolvedConfiguration> {
    let text = tokio::fs::read_to_string(path).await.map_err(|e| {
        Error::new(
            nous_core::ErrorOwner::Instance,
            nous_core::ErrorCode::InvalidRequest,
            "Invalid request",
        )
        .with_source(e)
    })?;
    let config: Config = toml::from_str(&text).map_err(|e| {
        Error::new(
            nous_core::ErrorOwner::Instance,
            nous_core::ErrorCode::InvalidRequest,
            "Invalid request",
        )
        .with_source(e)
    })?;
    if config.bootstrap.object_store.backend != "fs" {
        return Err(Error::new(
            nous_core::ErrorOwner::Instance,
            nous_core::ErrorCode::InvalidRequest,
            "object store backend must be fs",
        ));
    }
    let absolute = std::path::absolute(path).map_err(|e| {
        Error::new(
            nous_core::ErrorOwner::Instance,
            nous_core::ErrorCode::InvalidRequest,
            "Invalid request",
        )
        .with_source(e)
    })?;
    let root = absolute
        .parent()
        .ok_or_else(|| {
            Error::new(
                nous_core::ErrorOwner::Instance,
                nous_core::ErrorCode::InvalidRequest,
                "config parent required",
            )
        })?
        .to_path_buf();
    let bundle = read_bundle(&resolve_path(&root, &config.configuration_bundle)).await?;
    // Validate the complete catalog/document before acquiring database resources.
    nous_kernel::configuration_catalog(bundle.core_descriptors.clone())?
        .deployment_values(&bundle.deployment_document)?;
    Ok(ResolvedConfiguration {
        config,
        root,
        bundle,
    })
}

/// Open a caller-supplied offline database copy for the one-time vector
/// ownership conversion. Only the copied database and copied object root are
/// opened; normal Kernel startup never runs this conversion.
pub async fn open_embedding_upgrade_copy(
    path: &Path,
    copied_database_url: String,
    copied_object_root: &Path,
    serving_scratch_root: &Path,
) -> Result<NousRuntime> {
    if !copied_object_root.is_dir() {
        return Err(Error::new(
            nous_core::ErrorOwner::Instance,
            nous_core::ErrorCode::FailedPrecondition,
            "copied object root must already exist; refusing to create or use the source root",
        ));
    }
    let loaded = read_configuration(path).await?;
    NousRuntime::open(RuntimeOptions {
        postgres_url: copied_database_url,
        max_connections: loaded.config.bootstrap.database.max_connections,
        acquire_timeout_ms: loaded.config.bootstrap.database.acquire_timeout_ms,
        object_root: std::fs::canonicalize(copied_object_root)
            .map_err(|error| {
                Error::new(
                    nous_core::ErrorOwner::Instance,
                    nous_core::ErrorCode::InternalFailure,
                    "Copied object root could not be resolved",
                )
                .with_source(error)
            })?
            .to_string_lossy()
            .into_owned(),
        serving_options: nous_retrieval::ServingOptions {
            root: serving_scratch_root.to_path_buf(),
            lexical: false,
            dense: false,
            topology: false,
            memory_enabled: false,
        },
        // This offline conversion reads already stored vectors and must never
        // invoke or initialize a host embedding provider.
        embedding: None,
        stored_embedding: None,
        core_descriptors: loaded.bundle.core_descriptors,
        deployment_document: loaded.bundle.deployment_document,
    })
    .await
}

async fn open_database(
    root: &Path,
    config: &DatabaseConfig,
) -> Result<(String, Option<PostgreSQL>)> {
    match config.mode.as_str() {
        "external" => {
            if config.url.trim().is_empty() {
                return Err(Error::new(
                    nous_core::ErrorOwner::Instance,
                    nous_core::ErrorCode::InvalidRequest,
                    "Invalid request",
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
                Error::new(
                    nous_core::ErrorOwner::Instance,
                    nous_core::ErrorCode::InternalFailure,
                    "Private PostgreSQL initialization failed",
                )
                .with_source(error)
            })?;
            postgres.start().await.map_err(|error| {
                Error::new(
                    nous_core::ErrorOwner::Instance,
                    nous_core::ErrorCode::InternalFailure,
                    "Private PostgreSQL startup failed",
                )
                .with_source(error)
            })?;
            let database = if config.name.trim().is_empty() {
                "nous_wave_20260908"
            } else {
                &config.name
            };
            if !postgres.database_exists(database).await.map_err(|error| {
                Error::new(
                    nous_core::ErrorOwner::Instance,
                    nous_core::ErrorCode::InternalFailure,
                    "Could not check whether the private PostgreSQL database exists",
                )
                .with_source(error)
            })? {
                postgres.create_database(database).await.map_err(|error| {
                    Error::new(
                        nous_core::ErrorOwner::Instance,
                        nous_core::ErrorCode::InternalFailure,
                        "Could not create the private PostgreSQL database",
                    )
                    .with_source(error)
                })?;
            }
            Ok((postgres.settings().url(database), Some(postgres)))
        }
        other => Err(Error::new(
            nous_core::ErrorOwner::Instance,
            nous_core::ErrorCode::InvalidRequest,
            "Unsupported database mode",
        )
        .with_context("mode", other)),
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
            return Err(Error::new(
                nous_core::ErrorOwner::Instance,
                nous_core::ErrorCode::Unavailable,
                "Service unavailable",
            ));
        }
    }
    let instance_dir = resolve_path(root, &config.instance_dir);
    let secret_dir = resolve_path(root, &config.secret_dir);
    tokio::fs::create_dir_all(&instance_dir)
        .await
        .map_err(|error| {
            Error::new(
                nous_core::ErrorOwner::Instance,
                nous_core::ErrorCode::InternalFailure,
                "Could not create the managed PostgreSQL instance directory",
            )
            .with_source(error)
        })?;
    tokio::fs::create_dir_all(&secret_dir)
        .await
        .map_err(|error| {
            Error::new(
                nous_core::ErrorOwner::Instance,
                nous_core::ErrorCode::InternalFailure,
                "Could not create the managed PostgreSQL secret directory",
            )
            .with_source(error)
        })?;
    if let Some(parent) = data_dir.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|error| {
            Error::new(
                nous_core::ErrorOwner::Instance,
                nous_core::ErrorCode::InternalFailure,
                "Could not create the managed PostgreSQL data parent directory",
            )
            .with_source(error)
        })?;
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
            let mut file = options.open(&password_file).await.map_err(|error| {
                Error::new(
                    nous_core::ErrorOwner::Instance,
                    nous_core::ErrorCode::InternalFailure,
                    "Could not create the managed PostgreSQL password file",
                )
                .with_source(error)
            })?;
            tokio::io::AsyncWriteExt::write_all(&mut file, value.as_bytes())
                .await
                .map_err(|error| {
                    Error::new(
                        nous_core::ErrorOwner::Instance,
                        nous_core::ErrorCode::InternalFailure,
                        "Could not write the managed PostgreSQL password file",
                    )
                    .with_source(error)
                })?;
            value
        }
        Err(error) => {
            return Err(Error::new(
                nous_core::ErrorOwner::Instance,
                nous_core::ErrorCode::InternalFailure,
                "Could not read the managed PostgreSQL password file",
            )
            .with_source(error));
        }
    };
    if password.is_empty() {
        return Err(Error::new(
            nous_core::ErrorOwner::Instance,
            nous_core::ErrorCode::InvalidRequest,
            "Invalid request",
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
        Ok(bytes) => serde_json::from_slice::<DatabaseState>(&bytes).map_err(|error| {
            Error::new(
                nous_core::ErrorOwner::Instance,
                nous_core::ErrorCode::InvalidRequest,
                "Private PostgreSQL state is invalid",
            )
            .with_source(error)
        })?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .map_err(|error| {
                    Error::new(
                        nous_core::ErrorOwner::Instance,
                        nous_core::ErrorCode::InternalFailure,
                        "Could not reserve a loopback port for managed PostgreSQL",
                    )
                    .with_source(error)
                })?;
            let value = DatabaseState {
                port: listener
                    .local_addr()
                    .map_err(|error| {
                        Error::new(
                            nous_core::ErrorOwner::Instance,
                            nous_core::ErrorCode::InternalFailure,
                            "Could not resolve the managed PostgreSQL listener address",
                        )
                        .with_source(error)
                    })?
                    .port(),
                major: 18,
            };
            tokio::fs::write(
                &state_file,
                serde_json::to_vec(&value).map_err(|error| {
                    Error::new(
                        nous_core::ErrorOwner::Instance,
                        nous_core::ErrorCode::InternalFailure,
                        "Could not encode managed PostgreSQL state",
                    )
                    .with_source(error)
                })?,
            )
            .await
            .map_err(|error| {
                Error::new(
                    nous_core::ErrorOwner::Instance,
                    nous_core::ErrorCode::InternalFailure,
                    "Could not persist managed PostgreSQL state",
                )
                .with_source(error)
            })?;
            value
        }
        Err(error) => {
            return Err(Error::new(
                nous_core::ErrorOwner::Instance,
                nous_core::ErrorCode::InternalFailure,
                "Could not read managed PostgreSQL state",
            )
            .with_source(error));
        }
    };
    if state.port == 0 || state.major != 18 {
        return Err(Error::new(
            nous_core::ErrorOwner::Instance,
            nous_core::ErrorCode::FailedPrecondition,
            "Failed precondition",
        ));
    }
    match tokio::fs::read_to_string(data_dir.join("PG_VERSION")).await {
        Ok(version) if version.trim() != "18" => {
            return Err(Error::new(
                nous_core::ErrorOwner::Instance,
                nous_core::ErrorCode::FailedPrecondition,
                "Failed precondition",
            ));
        }
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            return Err(Error::new(
                nous_core::ErrorOwner::Instance,
                nous_core::ErrorCode::InternalFailure,
                "Could not inspect the managed PostgreSQL data version",
            )
            .with_source(error));
        }
        _ => {}
    }
    let settings = SettingsBuilder::new()
        .version(VersionReq::parse("=18.6.0").map_err(|error| {
            Error::new(
                nous_core::ErrorOwner::Instance,
                nous_core::ErrorCode::InvalidRequest,
                "Invalid request",
            )
            .with_source(error)
        })?)
        .host("127.0.0.1")
        .config("listen_addresses", "127.0.0.1")
        .config("unix_socket_directories", "''")
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
        postgres.stop().await.map_err(|error| {
            Error::new(
                nous_core::ErrorOwner::Instance,
                nous_core::ErrorCode::InternalFailure,
                "Could not stop the managed PostgreSQL server",
            )
            .with_source(error)
        })?;
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
    let bytes = tokio::fs::read(path).await.map_err(|error| {
        Error::new(
            nous_core::ErrorOwner::Instance,
            nous_core::ErrorCode::InvalidRequest,
            "Invalid request",
        )
        .with_source(error)
    })?;
    let bundle: nous_configuration::ConfigurationBootstrapBundle = serde_json::from_slice(&bytes)
        .map_err(|error| {
        Error::new(
            nous_core::ErrorOwner::Instance,
            nous_core::ErrorCode::InvalidRequest,
            "invalid private configuration bundle",
        )
        .with_source(error)
    })?;
    Ok(bundle)
}

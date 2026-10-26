//! Process lifecycle and connecting a client to an existing instance.
use crate::http::{self, ApiClient, ApiState};
use anyhow::{Context, Result};
use nous_app::App;
use nous_foundation::instance::InstanceConfig;
use nous_foundation::instance::InstanceGuard;
use nous_foundation::instance::InstancePaths;
use nous_foundation::network::NetworkClients;
use nous_foundation::secret::Secrets;
use serde_json::{Value, json};
use std::{net::SocketAddr, path::PathBuf, sync::Arc};
use tokio_util::sync::CancellationToken;
pub(crate) async fn connect(
    paths: &InstancePaths,
    url: Option<String>,
    token_file: Option<PathBuf>,
) -> Result<ApiClient> {
    let config = if paths.config.exists() {
        Some(InstanceConfig::load(paths)?)
    } else {
        None
    };
    let running_state = if url.is_none() && InstanceGuard::is_running(paths)? {
        std::fs::read(paths.resolve("run/host.json")?)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
    } else {
        None
    };
    let running_address = running_state
        .as_ref()
        .and_then(|state| state.get("listen"))
        .and_then(Value::as_str)
        .and_then(|address| address.parse::<SocketAddr>().ok());
    let active_credential = running_state
        .as_ref()
        .and_then(|state| state.get("bootstrap_credential"))
        .map(|value| serde_json::from_value::<nous_domain::SecretRef>(value.clone()))
        .transpose()?;
    let base_url = url
        .or_else(|| {
            running_address
                .or_else(|| config.as_ref().map(|c| c.server.listen))
                .map(|mut address| {
                    if address.ip().is_unspecified() {
                        address.set_ip(if address.is_ipv4() {
                            std::net::Ipv4Addr::LOCALHOST.into()
                        } else {
                            std::net::Ipv6Addr::LOCALHOST.into()
                        });
                    }
                    format!("http://{address}")
                })
        })
        .context("instance is not initialized; pass --url and --token-file or run nous init")?;
    let token = if let Some(path) = token_file {
        Secrets::read_token_file(&path).await?.into_string()
    } else {
        let reference = active_credential.as_ref()
            .context("a running local identity or explicit --token-file is required; bootstrap seed is for local initialization/reset")?;
        Secrets::new(Arc::new(paths.clone()))
            .read(reference)
            .await?
            .into_string()
    };
    let network = running_state
        .as_ref()
        .and_then(|state| state.get("network"))
        .map(|value| serde_json::from_value(value.clone()))
        .transpose()?
        .or_else(|| config.map(|c| c.network))
        .unwrap_or_default();
    ApiClient::new(NetworkClients::new(&network)?, base_url, token)
}

pub(crate) async fn serve(paths: InstancePaths, listen: Option<SocketAddr>) -> Result<()> {
    anyhow::ensure!(
        paths.config.exists(),
        "instance is not initialized; run nous init"
    );
    paths.ensure_layout()?;
    let _guard = InstanceGuard::acquire(&paths)?;
    let app = App::open(paths.clone()).await?;
    let mut config = app.bootstrap_config().clone();
    nous_foundation::observability::Observability::current()?
        .configure(&paths, &config.observability)?;
    if let Some(listen) = listen {
        config.server.listen = listen;
    }
    let listener = tokio::net::TcpListener::bind(config.server.listen)
        .await
        .context("cannot bind instance listener")?;
    let address = listener.local_addr()?;
    app.activate_bootstrap_configuration(&config.server).await?;
    let cancellation = CancellationToken::new();
    let router = http::router(
        ApiState {
            app: app.clone(),
            shutdown: cancellation.clone(),
        },
        &config,
        cancellation.child_token(),
    );
    let run_path = paths.resolve("run/host.json")?;
    std::fs::write(
        &run_path,
        serde_json::to_vec_pretty(
            &json!({"pid":std::process::id(),"version":env!("CARGO_PKG_VERSION"),"listen":address,"bootstrap_credential":app.bootstrap_credential().await?,"network":config.network}),
        )?,
    )?;
    tracing::info!(instance=%paths.root.display(),listen=%address,"Nous Wave is serving");
    let background = app.start_background(cancellation.child_token());
    // Axum startup and cancellation wiring adapted from rmcp counter_streamhttp.rs.
    let shutdown = cancellation.clone();
    let result = axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            tokio::select! { _=shutdown_signal()=>{}, _=shutdown.cancelled()=>{} }
            shutdown.cancel();
        })
        .await;
    cancellation.cancel();
    background
        .await
        .context("background runtime stopped unexpectedly")?;
    app.close().await;
    let _ = std::fs::remove_file(run_path);
    result?;
    tracing::info!("Nous Wave stopped");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut terminate) => {
                tokio::select! { _=ctrl_c=>{}, _=terminate.recv()=>{} }
            }
            Err(_) => ctrl_c.await,
        }
    }
    #[cfg(not(unix))]
    ctrl_c.await;
}

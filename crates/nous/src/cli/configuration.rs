//! Online configuration management and offline bootstrap schema validation.
use super::{connect, print_json, scope::ScopeArgs};
use anyhow::{Context, Result, ensure};
use clap::Subcommand;
use nous_app::common::{
    configuration::{ConfigChangeRequest, ConfigEdit, ConfigRequest},
    instance::{InstanceConfig, InstancePaths},
};
use std::path::PathBuf;

#[derive(Subcommand)]
pub(super) enum ConfigCommand {
    /// Show both effective snapshots and sources; defaults to this credential's scope.
    Show {
        #[command(flatten)]
        scope: ScopeArgs,
    },
    /// Inspect registered typed configuration keys, defaults and application modes.
    Catalog {
        #[command(flatten)]
        scope: ScopeArgs,
    },
    /// Read immutable changes for a scope and optional key.
    History {
        #[command(flatten)]
        scope: ScopeArgs,
        #[arg(long)]
        key: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Apply JSON {operation_id, scope, edits:[{op, key, expected_revision, value}]} atomically.
    Set {
        #[arg(long)]
        file: PathBuf,
    },
    /// Apply a JSON batch containing only {op:"clear", key, expected_revision} edits.
    Clear {
        #[arg(long)]
        file: PathBuf,
    },
    /// Print the bootstrap instance.toml schema without connecting to a host.
    Schema,
    /// Validate the bootstrap file without opening Authority or making model calls.
    Validate,
}
impl Default for ConfigCommand {
    fn default() -> Self {
        Self::Show {
            scope: ScopeArgs::default(),
        }
    }
}

pub(super) async fn execute(
    paths: &InstancePaths,
    url: Option<String>,
    token_file: Option<PathBuf>,
    command: ConfigCommand,
) -> Result<()> {
    let request = match command {
        ConfigCommand::Schema => {
            return print_json(serde_json::to_value(schemars::schema_for!(InstanceConfig))?);
        }
        ConfigCommand::Validate => {
            InstanceConfig::load(paths)?;
            return print_json(serde_json::json!({"valid":true,"config":paths.config}));
        }
        ConfigCommand::Show { scope } => ConfigRequest::Show {
            scope: scope.scope(),
        },
        ConfigCommand::Catalog { scope } => ConfigRequest::Catalog {
            scope: scope.scope(),
        },
        ConfigCommand::History { scope, key, limit } => ConfigRequest::History {
            scope: scope.scope(),
            key,
            limit,
        },
        ConfigCommand::Set { file } => ConfigRequest::Change(read_change(file)?),
        ConfigCommand::Clear { file } => {
            let input = read_change(file)?;
            ensure!(
                input
                    .edits
                    .iter()
                    .all(|edit| matches!(edit, ConfigEdit::Clear { .. })),
                "config clear accepts only clear edits"
            );
            ConfigRequest::Change(input)
        }
    };
    print_json(
        connect(paths, url, token_file)?
            .execute(&crate::http::Operation::Configuration(request))
            .await?,
    )
}
fn read_change(file: PathBuf) -> Result<ConfigChangeRequest> {
    ensure!(
        std::fs::metadata(&file)?.len() <= 64 * 1024 * 1024,
        "configuration input exceeds 64 MiB"
    );
    serde_json::from_slice(&std::fs::read(&file)?)
        .with_context(|| format!("invalid configuration change file: {}", file.display()))
}

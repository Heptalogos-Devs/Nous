//! Command parsing and presentation; application decisions remain in App services.
pub mod bundle;
pub mod cognition;
mod configuration;
mod embedding;
mod identity;
pub mod maintenance;
mod materials;
mod media;
mod models;
mod prompts;
mod scope;
mod secrets;
use crate::{
    bootstrap::{connect, serve},
    http::{CancelJob, CreateNamespace, CreateSubject, Operation, RecordObservation, WireTime},
    mcp,
};
use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use cognition::{MemoryCommand, ModelCommand, QueryArgs};
use configuration::ConfigCommand;
use materials::MaterialCommand;
use nous_app::App;
use nous_domain::{CallContext, NamespaceId, SubjectId};
use nous_foundation::instance::InstanceGuard;
use nous_foundation::instance::InstancePaths;
use serde_json::{Value, json};
use std::{net::SocketAddr, path::PathBuf};
#[derive(Parser)]
#[command(version, about = "Nous Wave native cognition service")]
pub(crate) struct Cli {
    /// Instance directory. All persistent and temporary product data stays here.
    #[arg(long, global = true, env = "NOUS_INSTANCE")]
    instance: Option<PathBuf>,
    /// Service address for online operations; defaults to the instance listener.
    #[arg(long, global = true, env = "NOUS_URL")]
    url: Option<String>,
    /// Protected file containing a bearer token. Secrets are never command arguments.
    #[arg(long, global = true, env = "NOUS_TOKEN_FILE")]
    token_file: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Import protected credentials into the selected local instance.
    Secret {
        #[command(subcommand)]
        command: secrets::SecretCommand,
    },
    /// Manage immutable Prompt resources and exact role bindings.
    Prompt {
        #[command(subcommand)]
        command: prompts::PromptCommand,
    },
    /// Execute a typed cognition request from JSON, or print its JSON Schema.
    Cognition {
        #[arg(long, conflicts_with = "schema", required_unless_present = "schema")]
        file: Option<PathBuf>,
        #[arg(long)]
        schema: bool,
    },
    /// Import bounded source records; returns per-item acceptance and formation jobs.
    Import {
        #[command(subcommand)]
        command: maintenance::ImportCommand,
    },
    /// Initialize an instance and its protected administrator credential.
    Init,
    /// Start the sole runtime host for this instance.
    Serve {
        /// Override instance.toml listener for this invocation.
        #[arg(long)]
        listen: Option<SocketAddr>,
    },
    /// Read the running service's status. Offline instances report stopped without opening storage.
    Status,
    /// Inspect active log destinations and process diagnostics.
    Diagnostics,
    /// Follow committed operations, durable jobs and model attempts for one trace.
    Trace {
        trace_id: nous_domain::TraceId,
        /// next_cursor JSON returned by the previous page.
        #[arg(long)]
        cursor: Option<String>,
        #[arg(long, default_value_t = 100)]
        limit: usize,
    },
    /// Gracefully stop the running instance; administrator credential required.
    Stop,
    /// Show all resolved managed paths without opening storage.
    Paths,
    /// Manage scoped configuration; bootstrap schema and validation remain offline.
    Config {
        #[command(subcommand)]
        command: Option<ConfigCommand>,
    },
    /// Manage live grants and controlled delegation, or reset the stopped bootstrap owner.
    Identity {
        #[command(subcommand)]
        command: identity::IdentityCommand,
    },
    /// Create a complete snapshot or restore it into a new instance.
    Backup {
        #[command(subcommand)]
        command: BackupCommand,
    },
    Namespace {
        #[command(subcommand)]
        command: NamespaceCommand,
    },
    Subject {
        #[command(subcommand)]
        command: SubjectCommand,
    },
    Material {
        #[command(subcommand)]
        command: MaterialCommand,
    },
    /// Manage media sources, regions and persistent interpretation jobs.
    Media {
        #[command(subcommand)]
        command: media::MediaCommand,
    },
    Memory {
        #[command(subcommand)]
        command: MemoryCommand,
    },
    /// Manage the shared text and media vector workload.
    Embedding {
        #[command(subcommand)]
        command: embedding::EmbeddingCommand,
    },
    Query(QueryArgs),
    Model {
        #[command(subcommand)]
        command: ModelCommand,
    },
    Observation {
        #[command(subcommand)]
        command: ObservationCommand,
    },
    Job {
        #[command(subcommand)]
        command: JobCommand,
    },
    /// Read immutable source material using its stable material ID.
    Source {
        #[command(subcommand)]
        command: SourceCommand,
    },
    /// Bridge a stdio MCP client to the existing service; opens no database.
    Mcp {
        #[command(subcommand)]
        command: McpCommand,
    },
}

#[derive(Subcommand)]
enum BackupCommand {
    Create,
    /// Restore into the empty target selected with --instance; creates a new admin credential.
    Restore {
        source: PathBuf,
    },
}
#[derive(Subcommand)]
enum NamespaceCommand {
    Create {
        name: String,
        #[arg(long)]
        operation_id: Option<String>,
    },
    List,
}
#[derive(Subcommand)]
enum SubjectCommand {
    Create {
        #[arg(long)]
        namespace: NamespaceId,
        #[arg(long, default_value = "default")]
        name: String,
        #[arg(long, default_value = "memory")]
        capability: Vec<String>,
        #[arg(long)]
        operation_id: Option<String>,
    },
    List {
        #[arg(long)]
        namespace: NamespaceId,
    },
}
#[derive(Subcommand)]
enum ObservationCommand {
    Record {
        #[arg(long)]
        subject: SubjectId,
        #[arg(long)]
        material: String,
        /// JSON time point; omitted means the contact time was not supplied.
        #[arg(long)]
        observed_at: Option<String>,
        #[arg(long)]
        operation_id: Option<String>,
    },
    List {
        #[arg(long)]
        subject: SubjectId,
    },
}
#[derive(Subcommand)]
enum JobCommand {
    Get {
        id: String,
    },
    /// Cancel accepted work in its authorized scope.
    Cancel {
        id: String,
        #[arg(long)]
        operation_id: Option<String>,
    },
}
#[derive(Subcommand)]
enum SourceCommand {
    Read { id: String },
}
#[derive(Subcommand)]
enum McpCommand {
    Stdio,
}

pub(crate) async fn execute(cli: Cli) -> Result<()> {
    if let Command::Import {
        command:
            maintenance::ImportCommand::Bundle {
                command: bundle::BundleCommand::Inspect { file, details },
            },
    } = &cli.command
    {
        return print_json(bundle::inspect(file, *details)?);
    }
    let root = cli
        .instance
        .clone()
        .map(Ok)
        .unwrap_or_else(InstancePaths::default_path)?;
    let paths = InstancePaths::new(root)?;
    match cli.command {
        Command::Secret { command } => {
            anyhow::ensure!(cli.url.is_none(), "secret import uses local --instance");
            command.execute(paths).await?;
        }
        Command::Cognition { schema: true, .. } => print_json(serde_json::to_value(
            schemars::schema_for!(crate::http::structured::StructuredOperation),
        )?)?,
        Command::Init => {
            paths.ensure_layout()?;
            let _guard = InstanceGuard::acquire(&paths)?;
            let created = nous_foundation::instance::initialize(&paths)?;
            let app = App::open(paths.clone()).await?;
            let credential = app.bootstrap_credential().await;
            app.close().await;
            let credential = credential?;
            print_json(
                json!({"state":if created {"initialized"} else {"already_initialized"},"instance":paths.root,"config":paths.config,"administrator_token_file":paths.resolve(&credential.file)?}),
            )?;
        }
        Command::Serve { listen } => serve(paths, listen).await?,
        Command::Backup {
            command: BackupCommand::Restore { source },
        } => print_json(nous_app::backup::restore(&source, paths).await?)?,
        Command::Stop => print_json(
            connect(&paths, cli.url, cli.token_file)
                .await?
                .stop()
                .await?,
        )?,
        Command::Diagnostics => print_json(
            connect(&paths, cli.url, cli.token_file)
                .await?
                .execute(&Operation::Diagnostics)
                .await?,
        )?,
        Command::Paths => print_json(serde_json::to_value(paths)?)?,
        Command::Trace {
            trace_id,
            cursor,
            limit,
        } => {
            let request = nous_app::administration::observability::TraceRequest {
                trace_id,
                cursor: cursor
                    .map(|value| serde_json::from_str(&value))
                    .transpose()?,
                limit,
            };
            print_json(
                connect(&paths, cli.url, cli.token_file)
                    .await?
                    .execute(&Operation::Trace(request))
                    .await?,
            )?;
        }
        Command::Config { command } => {
            configuration::execute(&paths, cli.url, cli.token_file, command.unwrap_or_default())
                .await?
        }
        Command::Material { command } => {
            command
                .execute(connect(&paths, cli.url, cli.token_file).await?)
                .await?;
        }
        Command::Media { command } => match command.operation()? {
            Some(operation) => print_json(
                connect(&paths, cli.url, cli.token_file)
                    .await?
                    .execute(&Operation::Media(Box::new(operation)))
                    .await?,
            )?,
            None => print_json(serde_json::to_value(schemars::schema_for!(
                crate::http::media::MediaOperation
            ))?)?,
        },
        Command::Model {
            command: ModelCommand::Check,
        } => {
            let _guard = InstanceGuard::acquire(&paths)?;
            let app = nous_app::App::open(paths).await?;
            let value = app.model_status(&nous_domain::CallContext::admin()).await?;
            app.close().await;
            print_json(value)?;
        }
        Command::Model {
            command: command @ (ModelCommand::Contract { .. } | ModelCommand::Preview { .. }),
        } => {
            let operation = models::operation(command)?;
            print_json(
                connect(&paths, cli.url, cli.token_file)
                    .await?
                    .execute(&operation)
                    .await?,
            )?;
        }
        Command::Status => {
            // A local stopped instance needs no token and no SQLite connection.
            if cli.url.is_none() && !InstanceGuard::is_running(&paths)? {
                print_json(
                    json!({"state":"stopped","instance":paths.root,"initialized":paths.config.exists()}),
                )?;
            } else {
                let client = connect(&paths, cli.url, cli.token_file).await?;
                print_json(client.execute(&Operation::Status).await?)?;
            }
        }
        Command::Identity {
            command:
                identity::IdentityCommand::ResetBootstrap {
                    reason,
                    operation_id,
                },
        } => {
            anyhow::ensure!(
                cli.url.is_none(),
                "bootstrap reset uses local --instance; --url is not accepted"
            );
            paths.ensure_layout()?;
            let _guard = InstanceGuard::acquire(&paths)?;
            let app = App::open(paths.clone()).await?;
            let result = app
                .reset_bootstrap(&CallContext::admin(), &op_id(operation_id), &reason)
                .await;
            app.close().await;
            print_json(serde_json::to_value(result?)?)?;
        }
        Command::Mcp {
            command: McpCommand::Stdio,
        } => {
            use rmcp::ServiceExt;
            let client = connect(&paths, cli.url, cli.token_file).await?;
            let service = mcp::McpService::remote(client)
                .serve(rmcp::transport::stdio())
                .await?;
            service.waiting().await?;
        }
        command => {
            let operation = match command {
                Command::Import { command } => command.operation()?,
                Command::Identity { command } => command.operation()?,
                Command::Cognition {
                    file: Some(file), ..
                } => {
                    let input =
                        std::fs::read_to_string(&file).context("cannot read cognition request")?;
                    Operation::Structured(
                        serde_json::from_str(&input).context("invalid typed cognition request")?,
                    )
                }
                Command::Backup {
                    command: BackupCommand::Create,
                } => Operation::CreateBackup,
                Command::Memory { command } => command.operation()?,
                Command::Embedding { command } => command.operation(),
                Command::Query(input) => input.operation()?,
                Command::Prompt { command } => command.operation()?,
                Command::Model {
                    command: ModelCommand::Status,
                } => Operation::Cognition(crate::http::cognition::CognitionOperation::ModelStatus),
                Command::Model {
                    command: ModelCommand::Call { id },
                } => Operation::Cognition(crate::http::cognition::CognitionOperation::ModelCall(
                    id.to_string(),
                )),
                Command::Namespace {
                    command: NamespaceCommand::Create { name, operation_id },
                } => Operation::CreateNamespace(CreateNamespace {
                    operation_id: op_id(operation_id),
                    name,
                }),
                Command::Namespace {
                    command: NamespaceCommand::List,
                } => Operation::ListNamespaces,
                Command::Subject {
                    command:
                        SubjectCommand::Create {
                            namespace,
                            name,
                            capability,
                            operation_id,
                        },
                } => Operation::CreateSubject(
                    namespace.to_string(),
                    CreateSubject {
                        operation_id: op_id(operation_id),
                        name,
                        capabilities: capability,
                    },
                ),
                Command::Subject {
                    command: SubjectCommand::List { namespace },
                } => Operation::ListSubjects(namespace.to_string()),
                Command::Source {
                    command: SourceCommand::Read { id },
                } => {
                    let _: nous_domain::MaterialId = id.parse()?;
                    Operation::GetMaterial(id)
                }
                Command::Observation {
                    command:
                        ObservationCommand::Record {
                            subject,
                            material,
                            observed_at,
                            operation_id,
                        },
                } => {
                    let _: nous_domain::MaterialId = material.parse()?;
                    Operation::RecordObservation(RecordObservation {
                        operation_id: op_id(operation_id),
                        subject_id: subject.to_string(),
                        material_id: material,
                        observed_at: parse_time(observed_at)?,
                    })
                }
                Command::Observation {
                    command: ObservationCommand::List { subject },
                } => Operation::ListObservations(subject.to_string()),
                Command::Job {
                    command: JobCommand::Get { id },
                } => Operation::GetJob(id),
                Command::Job {
                    command: JobCommand::Cancel { id, operation_id },
                } => Operation::CancelJob(
                    id,
                    CancelJob {
                        operation_id: op_id(operation_id),
                    },
                ),
                _ => bail!("unsupported command"),
            };
            print_json(
                connect(&paths, cli.url, cli.token_file)
                    .await?
                    .execute(&operation)
                    .await?,
            )?;
        }
    }
    Ok(())
}

fn op_id(value: Option<String>) -> String {
    value.unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
}
fn parse_time(value: Option<String>) -> Result<Option<WireTime>> {
    value
        .map(|text| {
            serde_json::from_str(&text)
                .context("time must be a JSON object with domain and integer coordinate")
        })
        .transpose()
}
fn print_json(value: Value) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

//! Command parsing and presentation; application decisions remain in App services.
pub mod bundle;
pub mod cognition;
mod configuration;
pub mod maintenance;
mod models;
mod prompts;
mod scope;
use crate::{
    bootstrap::{connect, serve},
    http::{
        CancelJob, CreateNamespace, CreateSubject, Operation, PutMaterial, RecordObservation,
        WireTime,
    },
    mcp,
};
use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use cognition::{MemoryCommand, ModelCommand, QueryArgs};
use configuration::ConfigCommand;
use nous_app::App;
use nous_domain::{AccessScope, CallContext, NamespaceId, SubjectId};
use nous_foundation::instance::InstanceConfig;
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
    /// Manage scoped credentials while the instance is stopped.
    Token {
        #[command(subcommand)]
        command: TokenCommand,
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
    Memory {
        #[command(subcommand)]
        command: MemoryCommand,
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
enum TokenCommand {
    Create {
        #[arg(long)]
        name: String,
        #[arg(long)]
        namespace: NamespaceId,
        #[arg(long)]
        subject: Option<SubjectId>,
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
enum MaterialCommand {
    Put {
        #[arg(long)]
        namespace: NamespaceId,
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        source: Option<String>,
        /// JSON time point, for example {"domain":"utc","coordinate":123}.
        #[arg(long)]
        event_time: Option<String>,
        #[arg(long)]
        operation_id: Option<String>,
    },
    Get {
        id: String,
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
        Command::Cognition { schema: true, .. } => print_json(serde_json::to_value(
            schemars::schema_for!(crate::http::structured::StructuredOperation),
        )?)?,
        Command::Init => {
            paths.ensure_layout()?;
            let _guard = InstanceGuard::acquire(&paths)?;
            let created = paths.initialize()?;
            let app = App::open(paths.clone()).await?;
            app.close().await;
            print_json(
                json!({"state":if created {"initialized"} else {"already_initialized"},"instance":paths.root,"config":paths.config,"administrator_token_file":paths.credentials.join("admin.token")}),
            )?;
        }
        Command::Serve { listen } => serve(paths, listen).await?,
        Command::Backup {
            command: BackupCommand::Restore { source },
        } => print_json(nous_app::backup::restore(&source, paths).await?)?,
        Command::Stop => print_json(connect(&paths, cli.url, cli.token_file)?.stop().await?)?,
        Command::Diagnostics => print_json(
            connect(&paths, cli.url, cli.token_file)?
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
                connect(&paths, cli.url, cli.token_file)?
                    .execute(&Operation::Trace(request))
                    .await?,
            )?;
        }
        Command::Config { command } => {
            configuration::execute(&paths, cli.url, cli.token_file, command.unwrap_or_default())
                .await?
        }
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
                connect(&paths, cli.url, cli.token_file)?
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
                let client = connect(&paths, cli.url, cli.token_file)?;
                print_json(client.execute(&Operation::Status).await?)?;
            }
        }
        Command::Token {
            command:
                TokenCommand::Create {
                    name,
                    namespace,
                    subject,
                },
        } => {
            let _config = InstanceConfig::load(&paths)?;
            paths.ensure_layout()?;
            let _guard = InstanceGuard::acquire(&paths)?;
            let app = App::open(paths.clone()).await?;
            let namespaces = app.list_namespaces(&CallContext::admin()).await?;
            anyhow::ensure!(
                namespaces.iter().any(|value| value.id == namespace),
                "namespace not found"
            );
            let scope = if let Some(subject) = subject {
                let subjects = app.list_subjects(&CallContext::admin(), &namespace).await?;
                anyhow::ensure!(
                    subjects.iter().any(|value| value.id == subject),
                    "subject not found in this namespace"
                );
                AccessScope::Subject {
                    namespace_id: namespace,
                    subject_id: subject,
                }
            } else {
                AccessScope::Namespace {
                    namespace_id: namespace,
                }
            };
            let result = paths.issue_token(&name, scope)?;
            app.close().await;
            print_json(result)?;
        }
        Command::Mcp {
            command: McpCommand::Stdio,
        } => {
            use rmcp::ServiceExt;
            let client = connect(&paths, cli.url, cli.token_file)?;
            let service = mcp::McpService::remote(client)
                .serve(rmcp::transport::stdio())
                .await?;
            service.waiting().await?;
        }
        command => {
            let operation = match command {
                Command::Import { command } => command.operation()?,
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
                Command::Material {
                    command:
                        MaterialCommand::Put {
                            namespace,
                            file,
                            source,
                            event_time,
                            operation_id,
                        },
                } => {
                    let metadata = std::fs::metadata(&file)
                        .with_context(|| format!("cannot read {}", file.display()))?;
                    anyhow::ensure!(
                        metadata.len() <= 4 * 1024 * 1024,
                        "material file exceeds the 4 MiB CLI text limit"
                    );
                    let text = std::fs::read_to_string(&file)
                        .context("material file must contain UTF-8 text")?;
                    Operation::PutMaterial(PutMaterial {
                        operation_id: op_id(operation_id),
                        namespace_id: namespace.to_string(),
                        text,
                        source: source.or_else(|| Some(file.to_string_lossy().into_owned())),
                        event_time: parse_time(event_time)?,
                    })
                }
                Command::Material {
                    command: MaterialCommand::Get { id },
                }
                | Command::Source {
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
                connect(&paths, cli.url, cli.token_file)?
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

use super::{op_id, print_result};
use crate::http::{ApiClient, Operation, bundles::BundleOperation};
use anyhow::Result;
use clap::{Args, Subcommand};
use nous_app::bundles::{InspectBundleRequest, ResolveBundleFileRequest};
use nous_domain::*;

#[derive(Subcommand)]
pub enum BundleCommand {
    /// Export a subject archive or an A-Memorix portable knowledge bundle.
    Export {
        #[command(flatten)]
        options: ExportOptions,
        /// Show selected contents without publishing a bundle.
        #[arg(long)]
        preview: bool,
        #[arg(long)]
        operation_id: Option<String>,
    },
    /// Inspect an uploaded bundle without installing it.
    Inspect {
        #[arg(long)]
        namespace: NamespaceId,
        #[arg(long)]
        subject: SubjectId,
        #[arg(long)]
        material: MaterialId,
    },
    /// Install bundle bytes uploaded with material put --owner-subject.
    Install {
        #[arg(long)]
        namespace: NamespaceId,
        #[arg(long)]
        subject: SubjectId,
        #[arg(long)]
        material: MaterialId,
        #[arg(long, default_value = "merge", value_parser = parse_mode)]
        mode: BundleInstallMode,
        /// Bind installed knowledge to this conversation; otherwise publish globally.
        #[arg(long)]
        chat: Option<ConversationRef>,
        #[arg(long)]
        operation_id: Option<String>,
    },
    List {
        #[arg(long)]
        namespace: NamespaceId,
        #[arg(long)]
        subject: SubjectId,
    },
    Get {
        installation_id: String,
    },
    /// Resolve a generated file name to its authorized material download.
    ResolveFile {
        #[arg(long)]
        namespace: NamespaceId,
        #[arg(long)]
        subject: SubjectId,
        file_name: String,
    },
    Uninstall {
        installation_id: String,
        #[arg(long)]
        operation_id: Option<String>,
    },
}
#[derive(Args)]
pub struct ExportOptions {
    #[arg(long)]
    namespace: NamespaceId,
    #[arg(long)]
    subject: SubjectId,
    #[arg(long)]
    package_id: String,
    #[arg(long)]
    name: String,
    #[arg(long)]
    version: Option<String>,
    #[arg(long, default_value = "")]
    description: String,
    #[arg(long, default_value = "")]
    author: String,
    #[arg(long, default_value = "native", value_parser = parse_format)]
    format: BundleExportFormat,
    /// Native defaults to full; amemorix defaults to knowledge.
    #[arg(long, value_parser = parse_level)]
    content_level: Option<BundleContentLevel>,
    /// Typed selector JSON, for example {"kind":"source","sources":["notes"]}.
    #[arg(long, conflicts_with = "chat")]
    selector: Option<String>,
    #[arg(long)]
    chat: Option<ConversationRef>,
    #[arg(long)]
    include_vectors: bool,
    #[arg(long)]
    exclude_images: bool,
    #[arg(long)]
    include_image_related: bool,
}
impl ExportOptions {
    fn request(self) -> Result<ExportBundleRequest> {
        let selector = if let Some(value) = self.selector {
            serde_json::from_str(&value)?
        } else if let Some(conversation_ref) = self.chat {
            BundleSelector::Chat { conversation_ref }
        } else {
            BundleSelector::All
        };
        let assets = if self.format == BundleExportFormat::Amemorix
            || self.include_vectors
            || self.exclude_images
            || self.include_image_related
        {
            Some(BundleAssetOptions {
                include_vectors: self.include_vectors,
                include_images: !self.exclude_images,
                include_image_related: self.include_image_related,
            })
        } else {
            None
        };
        let request = ExportBundleRequest {
            namespace_id: self.namespace,
            subject_id: self.subject,
            package: PackageDescriptor {
                id: self.package_id,
                name: self.name,
                version: self.version,
                description: self.description,
                author: self.author,
            },
            content_level: self.content_level.unwrap_or(match self.format {
                BundleExportFormat::Native => BundleContentLevel::Full,
                BundleExportFormat::Amemorix => BundleContentLevel::Knowledge,
            }),
            format: self.format,
            selector,
            assets,
        };
        request.validate()?;
        Ok(request)
    }
}
impl BundleCommand {
    pub async fn execute(self, client: ApiClient) -> Result<()> {
        let operation = match self {
            Self::Export {
                options,
                preview,
                operation_id,
            } => {
                let request = options.request()?;
                if preview {
                    BundleOperation::Preview { request }
                } else {
                    BundleOperation::Export {
                        operation_id: op_id(operation_id),
                        request,
                    }
                }
            }
            Self::Inspect {
                namespace,
                subject,
                material,
            } => BundleOperation::Inspect {
                request: InspectBundleRequest {
                    namespace_id: namespace,
                    subject_id: subject,
                    material_id: material,
                },
            },
            Self::Install {
                namespace,
                subject,
                material,
                mode,
                chat,
                operation_id,
            } => BundleOperation::Install {
                operation_id: op_id(operation_id),
                request: InstallBundleRequest {
                    namespace_id: namespace,
                    subject_id: subject,
                    material_id: material,
                    mode,
                    placement: chat.map_or(BundlePlacement::Global, |conversation_ref| {
                        BundlePlacement::Chat { conversation_ref }
                    }),
                },
            },
            Self::List { namespace, subject } => BundleOperation::List {
                namespace_id: namespace,
                subject_id: subject,
            },
            Self::Get { installation_id } => BundleOperation::Get { installation_id },
            Self::ResolveFile {
                namespace,
                subject,
                file_name,
            } => BundleOperation::ResolveFile {
                request: ResolveBundleFileRequest {
                    namespace_id: namespace,
                    subject_id: subject,
                    file_name,
                },
            },
            Self::Uninstall {
                installation_id,
                operation_id,
            } => BundleOperation::Uninstall {
                installation_id,
                operation_id: op_id(operation_id),
            },
        };
        print_result(
            client
                .execute(&Operation::Bundle(Box::new(operation)))
                .await?,
        )
    }
}
fn parse_mode(value: &str) -> Result<BundleInstallMode, String> {
    serde_json::from_value(serde_json::json!(value)).map_err(|error| error.to_string())
}
fn parse_format(value: &str) -> Result<BundleExportFormat, String> {
    serde_json::from_value(serde_json::json!(value)).map_err(|error| error.to_string())
}
fn parse_level(value: &str) -> Result<BundleContentLevel, String> {
    serde_json::from_value(serde_json::json!(value)).map_err(|error| error.to_string())
}

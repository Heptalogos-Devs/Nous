use super::{op_id, print_result};
use crate::http::{ApiClient, Operation, bundles::BundleOperation};
use anyhow::Result;
use clap::Subcommand;
use nous_domain::{
    BundleInstallMode, ExportBundleRequest, InstallBundleRequest, MaterialId, NamespaceId,
    SubjectId,
};

#[derive(Subcommand)]
pub enum BundleCommand {
    /// Export subject knowledge; download download_material_id with material get --output.
    Export {
        #[arg(long)]
        namespace: NamespaceId,
        #[arg(long)]
        subject: SubjectId,
        #[arg(long)]
        package_id: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        operation_id: Option<String>,
    },
    /// Install archive bytes already uploaded using material put.
    Install {
        #[arg(long)]
        namespace: NamespaceId,
        #[arg(long)]
        subject: SubjectId,
        #[arg(long)]
        material: MaterialId,
        #[arg(long, default_value = "merge", value_parser = parse_mode)]
        mode: BundleInstallMode,
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
    Uninstall {
        installation_id: String,
        #[arg(long)]
        operation_id: Option<String>,
    },
}
impl BundleCommand {
    pub async fn execute(self, client: ApiClient) -> Result<()> {
        let operation = match self {
            Self::Export {
                namespace,
                subject,
                package_id,
                name,
                operation_id,
            } => BundleOperation::Export {
                operation_id: op_id(operation_id),
                request: ExportBundleRequest {
                    namespace_id: namespace,
                    subject_id: subject,
                    package_id,
                    name,
                },
            },
            Self::Install {
                namespace,
                subject,
                material,
                mode,
                operation_id,
            } => BundleOperation::Install {
                operation_id: op_id(operation_id),
                request: InstallBundleRequest {
                    namespace_id: namespace,
                    subject_id: subject,
                    material_id: material,
                    mode,
                },
            },
            Self::List { namespace, subject } => BundleOperation::List {
                namespace_id: namespace,
                subject_id: subject,
            },
            Self::Get { installation_id } => BundleOperation::Get { installation_id },
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

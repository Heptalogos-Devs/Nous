use super::print_json;
use anyhow::Result;
use clap::Subcommand;
use nous_domain::SecretRef;
use nous_foundation::{instance::InstancePaths, secret::Secrets};
use std::{path::PathBuf, sync::Arc};

#[derive(Subcommand)]
pub enum SecretCommand {
    /// Import a provider credential into the protected local instance store.
    Import {
        #[arg(long)]
        file: PathBuf,
        /// Relative destination inside credentials, for example credentials/new-api.token.
        #[arg(long)]
        reference: PathBuf,
    },
}
impl SecretCommand {
    pub async fn execute(self, paths: InstancePaths) -> Result<()> {
        match self {
            Self::Import { file, reference } => {
                let reference = SecretRef { file: reference };
                reference.validate()?;
                let value = Secrets::read_token_file(&file).await?;
                Secrets::new(Arc::new(paths))
                    .write_new(&reference, value)
                    .await?;
                print_json(serde_json::to_value(reference)?)
            }
        }
    }
}

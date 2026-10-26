use super::print_result;
use crate::http::{ApiClient, Operation, integrations::AmemorixRequest};
use anyhow::{Context, Result};
use clap::Subcommand;
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum AmemorixCommand {
    Schema,
    Apply { file: PathBuf },
}
impl AmemorixCommand {
    pub fn operation(self) -> Result<Option<AmemorixRequest>> {
        match self {
            Self::Schema => Ok(None),
            Self::Apply { file } => Ok(Some(
                serde_json::from_reader(std::fs::File::open(&file)?)
                    .with_context(|| format!("invalid A-Memorix operation {}", file.display()))?,
            )),
        }
    }
}
pub async fn execute(client: ApiClient, request: AmemorixRequest) -> Result<()> {
    print_result(
        client
            .execute(&Operation::Amemorix(Box::new(request)))
            .await?,
    )
}

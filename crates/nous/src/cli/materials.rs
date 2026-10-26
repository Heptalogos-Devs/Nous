use super::{op_id, print_json};
use crate::http::{ApiClient, Operation};
use anyhow::{Context, Result};
use clap::Subcommand;
use futures::TryStreamExt;
use nous_domain::{MaterialId, NamespaceId, TimePoint, media::PutMediaRequest};
use serde_json::json;
use std::path::PathBuf;
use tokio::io::AsyncWriteExt;
use tokio_util::io::StreamReader;

#[derive(Subcommand)]
pub enum MaterialCommand {
    /// Stream a text, image, audio, video or document file into the service.
    Put {
        #[arg(long)]
        namespace: NamespaceId,
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        source: Option<String>,
        /// MIME hint; the media service records the actual detected type.
        #[arg(long, default_value = "application/octet-stream")]
        media_type: String,
        /// JSON time point, for example {"domain":"utc","coordinate":123}.
        #[arg(long)]
        event_time: Option<String>,
        /// Optional expected SHA-256 digest of the original bytes.
        #[arg(long)]
        expected_content_hash: Option<String>,
        #[arg(long)]
        operation_id: Option<String>,
    },
    /// Read text and metadata, or download original bytes with --output.
    Get {
        id: MaterialId,
        #[arg(long)]
        output: Option<PathBuf>,
    },
}

impl MaterialCommand {
    pub async fn execute(self, client: ApiClient) -> Result<()> {
        match self {
            Self::Put {
                namespace,
                file,
                source,
                media_type,
                event_time,
                expected_content_hash,
                operation_id,
            } => {
                let reader = tokio::fs::File::open(&file)
                    .await
                    .with_context(|| format!("cannot open {}", file.display()))?;
                let length = reader.metadata().await?.len();
                let request = PutMediaRequest {
                    namespace_id: namespace,
                    media_type,
                    declared_byte_len: Some(length),
                    expected_content_hash,
                    source: source.or_else(|| Some(file.to_string_lossy().into_owned())),
                    event_time: event_time
                        .map(|value| serde_json::from_str::<TimePoint>(&value))
                        .transpose()
                        .context("event time must have domain and coordinate")?,
                };
                print_json(
                    client
                        .upload_material(op_id(operation_id), request, reader)
                        .await?,
                )
            }
            Self::Get { id, output: None } => print_json(
                client
                    .execute(&Operation::GetMaterial(id.to_string()))
                    .await?,
            ),
            Self::Get {
                id,
                output: Some(path),
            } => {
                let response = client.download_material(&id).await?;
                let headers = response.headers().clone();
                let mut output = tokio::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)
                    .await
                    .with_context(|| format!("cannot create {}", path.display()))?;
                let mut reader =
                    StreamReader::new(response.bytes_stream().map_err(std::io::Error::other));
                let copied = tokio::io::copy(&mut reader, &mut output).await;
                let flushed = output.flush().await;
                drop(output);
                let length = match copied.and_then(|length| flushed.map(|_| length)) {
                    Ok(length) => length,
                    Err(error) => {
                        if let Err(cleanup) = tokio::fs::remove_file(&path).await {
                            tracing::warn!(path=%path.display(),error=%cleanup,"cannot remove incomplete material download");
                        }
                        return Err(error).context("material download failed");
                    }
                };
                print_json(json!({"material_id":id,"output":path,"byte_len":length,
                    "content_hash":headers.get("x-nous-content-sha256").and_then(|value|value.to_str().ok()),
                    "media_type":headers.get("content-type").and_then(|value|value.to_str().ok())}))
            }
        }
    }
}

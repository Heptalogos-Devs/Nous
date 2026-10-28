use super::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResolveBundleFileRequest {
    pub namespace_id: NamespaceId,
    pub subject_id: SubjectId,
    pub file_name: String,
}

impl App {
    /// Resolve a generated artifact name to a scoped download; paths stay local to each client.
    pub async fn resolve_bundle_file(
        &self,
        ctx: &CallContext,
        request: ResolveBundleFileRequest,
        cancel: &CancellationToken,
    ) -> Result<Value> {
        ctx.require_subject(&request.namespace_id, &request.subject_id)?;
        let stem = request
            .file_name
            .strip_prefix("bundle-")
            .and_then(|s| {
                s.strip_suffix(".amembundle")
                    .or_else(|| s.strip_suffix(".nousbundle"))
            })
            .ok_or_else(|| Error::invalid("invalid generated bundle file name"))?;
        let id: MaterialId = stem.parse()?;
        let original = self.sources.original_metadata(ctx, &id, cancel).await?;
        let details = &original.details;
        if original.material.namespace_id != request.namespace_id
            || details.source_type.as_deref() != Some("nous_bundle")
            || details.metadata.get("subject_id").and_then(Value::as_str)
                != Some(request.subject_id.as_str())
            || details.metadata.get("file_name").and_then(Value::as_str)
                != Some(request.file_name.as_str())
        {
            return Err(Error::new(
                ErrorCode::NotFound,
                "bundle artifact was not found in this subject",
            ));
        }
        Ok(json!({
            "material_id":id,"file_name":request.file_name,"size":original.material.byte_len,
            "media_type":original.material.media_type,"export":details.metadata.get("export"),
        }))
    }
}

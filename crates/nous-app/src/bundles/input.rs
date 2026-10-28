//! Archive admission and staging shared by inspection and installation.
use super::*;
use crate::materials::sources::OriginalMetadata;
use std::path::PathBuf;

pub(super) struct BundleInput {
    pub original:OriginalMetadata,
    pub staging:tempfile::TempDir,
    pub path:PathBuf,
    pub policy:policy::BundlePolicy,
}
impl App {
    pub(super) async fn bundle_input(&self,ctx:&CallContext,namespace:&NamespaceId,subject:&SubjectId,material:&MaterialId,cancel:&CancellationToken)->Result<BundleInput> {
        ctx.require_subject(namespace,subject)?;
        let original=self.sources.original_metadata(ctx,material,cancel).await?;
        if original.material.namespace_id!=*namespace {return Err(Error::new(ErrorCode::Forbidden,"bundle archive is outside the target namespace"));}
        let policy=self.bundle_policy(namespace,subject).await?;
        if original.material.byte_len as u64>policy.limits.archive_bytes {
            return Err(Error::new(ErrorCode::ResourceExhausted,"bundle archive exceeds configured limit"));
        }
        let staging=self.bundle_staging(cancel).await?;
        let path=staging.path().join("uploaded.bundle");
        self.objects.copy_to(&original.material.content_hash,original.material.byte_len,&path,cancel).await?;
        Ok(BundleInput{original,staging,path,policy})
    }
}

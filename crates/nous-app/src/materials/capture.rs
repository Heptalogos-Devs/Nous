use super::sources::{OriginalMetadata, OriginalStream, SourceSnapshot};
use crate::App;
use nous_domain::{media::PutMediaRequest, *};
use nous_foundation::media::CapturedMedia;
use nous_storage::{BeginOperation, OperationReceipt, operation_key};
use serde_json::json;
use tokio::io::AsyncRead;
use tokio_util::sync::CancellationToken;

impl App {
    pub async fn inspect_material(
        &self,
        ctx: &CallContext,
        id: &MaterialId,
    ) -> Result<OriginalMetadata> {
        self.sources
            .original_metadata(ctx, id, &CancellationToken::new())
            .await
    }

    pub async fn read_media_source(
        &self,
        ctx: &CallContext,
        subject: &SubjectId,
        source: &media::MediaReference,
    ) -> Result<SourceSnapshot> {
        let namespace = self.database.subject_namespace(subject).await?;
        self.sources.resolve(ctx, &namespace, subject, source).await
    }

    pub async fn media_source_lineage(
        &self,
        ctx: &CallContext,
        subject: &SubjectId,
        source: &media::MediaReference,
    ) -> Result<media::MediaSourceLineage> {
        let namespace = self.database.subject_namespace(subject).await?;
        self.sources.lineage(ctx, &namespace, subject, source).await
    }

    pub async fn put_material(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        namespace_id: &NamespaceId,
        text: &str,
        source: Option<String>,
        event_time: Option<TimePoint>,
    ) -> Result<OperationReceipt> {
        let request = PutMediaRequest {
            namespace_id: namespace_id.clone(),
            owner_subject_id: None,
            media_type: "text/plain".into(),
            declared_byte_len: Some(text.len() as u64),
            expected_content_hash: None,
            source,
            event_time,
        };
        self.validate_material_admission(ctx, &request).await?;
        let captured = self
            .media
            .capture_text(text, &CancellationToken::new())
            .await?;
        self.admit_material(ctx, operation_id, request, captured)
            .await
    }

    pub async fn put_media<R: AsyncRead + Unpin + Send + 'static>(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        request: PutMediaRequest,
        reader: R,
        cancel: &CancellationToken,
    ) -> Result<OperationReceipt> {
        self.validate_material_admission(ctx, &request).await?;
        let captured = self
            .media
            .capture(ctx, reader, Some(request.media_type.clone()), cancel)
            .await?;
        self.admit_material(ctx, operation_id, request, captured)
            .await
    }

    async fn validate_material_admission(
        &self,
        ctx: &CallContext,
        request: &PutMediaRequest,
    ) -> Result<()> {
        request.validate()?;
        if let Some(subject) = &request.owner_subject_id {
            ctx.require_subject(&request.namespace_id, subject)?;
            self.database
                .subject(&request.namespace_id, subject)
                .await?;
        } else {
            ctx.require_namespace(&request.namespace_id)?;
            self.database.namespace(&request.namespace_id).await?;
        }
        Ok(())
    }

    async fn admit_material(
        &self,
        ctx: &CallContext,
        operation_id: &str,
        request: PutMediaRequest,
        captured: CapturedMedia,
    ) -> Result<OperationReceipt> {
        let CapturedMedia {
            object,
            media_type,
            metadata,
        } = captured;
        request.validate_received(object.byte_len as u64, &object.digest)?;
        let media_type = media::canonical_media_type(&media_type)?;
        let scope = match &request.owner_subject_id {
            Some(subject) => format!("namespace:{}/subject:{subject}", request.namespace_id),
            None => request.namespace_id.to_string(),
        };
        let key = operation_key(
            ctx,
            "material.put",
            &scope,
            operation_id,
            &json!({"namespace_id":request.namespace_id,"owner_subject_id":request.owner_subject_id,"content_hash":object.digest,
                "media_type":media_type,"source":request.source,"event_time":request.event_time}),
        )?;
        let mut tx = match self.database.begin_operation(key).await? {
            BeginOperation::Replay(receipt) => return Ok(receipt),
            BeginOperation::New(tx) => tx,
        };
        object.protect_in(&mut tx);
        tx.namespace(&request.namespace_id).await?;
        if let Some(subject) = &request.owner_subject_id {
            tx.context()
                .require_subject(&request.namespace_id, subject)?;
            tx.subject(&request.namespace_id, subject).await?;
        } else {
            tx.context().require_namespace(&request.namespace_id)?;
        }
        let material = Material {
            id: MaterialId::new(),
            namespace_id: request.namespace_id,
            content_hash: object.digest,
            media_type,
            metadata,
            byte_len: object.byte_len,
            source: request.source,
            event_time: request.event_time,
            recorded_at: tx.time().recorded(),
            commit_seq: tx.commit_seq(),
        };
        tx.insert_material(&material).await?;
        if let Some(subject) = &request.owner_subject_id {
            tx.retain_material_output(&material.namespace_id, subject, &material.id)
                .await?;
        }
        tx.commit(serde_json::to_value(material)?).await
    }

    pub async fn get_material(
        &self,
        ctx: &CallContext,
        id: &MaterialId,
    ) -> Result<MaterialContent> {
        self.sources
            .original_text(ctx, id, &CancellationToken::new())
            .await
    }

    pub async fn open_material(
        &self,
        ctx: &CallContext,
        id: &MaterialId,
        cancel: &CancellationToken,
    ) -> Result<OriginalStream> {
        self.sources.open_original(ctx, id, cancel).await
    }
}

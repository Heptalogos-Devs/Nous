use crate::App;
use nous_domain::*;
use nous_storage::SourceClosure;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct CognitiveSourceSnapshot {
    pub object: RetrievalRecord,
    pub closure: SourceClosure,
    pub watermark: i64,
}
impl App {
    pub async fn read_cognitive_source(
        &self,
        ctx: &CallContext,
        namespace: &NamespaceId,
        subject: &SubjectId,
        reference: &CognitiveRef,
    ) -> Result<CognitiveSourceSnapshot> {
        ctx.require_subject(namespace, subject)?;
        reference.validate()?;
        let mut view = self.database.read_view().await?;
        view.subject(namespace, subject).await?;
        view.require_source_read(
            ctx,
            namespace,
            subject,
            &RetrievalTarget::Cognition(reference.clone()),
        )
        .await?;
        let snapshot = view
            .cognitive_reference_snapshot(namespace, subject, "", reference, None, None)
            .await?;
        let selected = match (
            snapshot.memories.into_iter().next(),
            snapshot.structured.into_iter().next(),
        ) {
            (Some(entry), None) => Some((RetrievalRecord::Memory(entry.memory), entry.source)),
            (None, Some(entry)) => Some((RetrievalRecord::Structured(entry.object), entry.source)),
            _ => None,
        };
        let (object, closure) = selected
            .ok_or_else(|| Error::new(ErrorCode::NotFound, "cognitive source not found"))?;
        if object.deleted() || !closure.complete {
            return Err(Error::new(
                ErrorCode::NotFound,
                "cognitive source is no longer available",
            ));
        }
        for root in &closure.roots {
            if !view
                .subject_has_material_access(namespace, subject, &root.material_id)
                .await?
            {
                return Err(Error::new(
                    ErrorCode::Forbidden,
                    "cognitive source is outside the subject's admitted material",
                ));
            }
        }
        let watermark = snapshot.watermark;
        view.finish().await?;
        Ok(CognitiveSourceSnapshot {
            object,
            closure,
            watermark,
        })
    }
}

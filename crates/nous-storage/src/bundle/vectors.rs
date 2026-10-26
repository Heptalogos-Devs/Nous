use super::remap::Remap;
use crate::WriteTransaction;
use nous_domain::*;
impl WriteTransaction {
    pub(super) async fn install_bundle_vectors(
        &mut self,
        package: &SubjectPackage,
        installation: &BundleInstallation,
        map: &Remap,
    ) -> Result<()> {
        for space in &package.vectors.spaces {
            self.save_embedding_space(space).await?;
        }
        for source in &package.vectors.contents {
            let value = map.vector_content(source)?;
            let accepted = self.save_embedding_content(&value).await?;
            if accepted.vector != value.vector {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "imported vector differs from the subject's accepted value for this space and content",
                ));
            }
        }
        for source in &package.vectors.associations {
            let mut value = map.vector_association(source)?;
            let accepted = self
                .matching_embedding(
                    &installation.namespace_id,
                    &installation.subject_id,
                    &value.record.space_id,
                    &value.record.source_hash,
                )
                .await?
                .ok_or_else(crate::not_found)?;
            value.record.model_call_id = accepted.model_call_id;
            value.record.vector = accepted.vector;
            // A receiving subject's declared epistemic perspective may change the
            // projection. The original numeric content and producer remain authoritative.
            let status = match self
                .validate_embedding_input(
                    &installation.namespace_id,
                    &installation.subject_id,
                    &value.input,
                )
                .await
            {
                Ok(()) => {
                    self.import_embedding_association(&value).await?;
                    if value.retired {
                        "accepted_history"
                    } else {
                        "accepted"
                    }
                }
                Err(error) if error.code == ErrorCode::NotFound => "source_unavailable",
                Err(error) if matches!(error.code, ErrorCode::Conflict | ErrorCode::Forbidden) => {
                    "projection_changed"
                }
                Err(error) => return Err(error),
            };
            self.record_vector_disposition(
                &installation.id,
                &BundleVectorDisposition {
                    space_id: value.record.space_id,
                    source_hash: value.record.source_hash,
                    source_target: source.record.target.clone(),
                    mapped_target: value.record.target,
                    model_call_id: value.record.model_call_id,
                    status: status.into(),
                },
            )
            .await?;
        }
        Ok(())
    }
}

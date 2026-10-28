use crate::ReadView;
use nous_domain::{media::*, *};

impl ReadView {
    pub(super) async fn authorize_bundle_snapshot(
        &mut self,
        ctx: &CallContext,
        package: &SubjectPackage,
    ) -> Result<()> {
        let ns = &package.subject.namespace_id;
        let subject = &package.subject.id;
        for source in &package.input_sources {
            // This explicit Original check is independent of each record's Memory grant.
            self.require_source_requirements(
                ctx,
                ns,
                subject,
                &[SourceReadRef::Media {
                    reference: MediaReference::Original {
                        material_id: source.material_id.clone(),
                    },
                    observation_id: None,
                }],
            )
            .await?;
        }
        for v in &package.materials {
            let observation_id = package
                .observations
                .iter()
                .find(|event| event.material_id == v.material.id)
                .map(|event| event.id.clone());
            self.require_source_requirements(
                ctx,
                ns,
                subject,
                &[SourceReadRef::Media {
                    reference: MediaReference::Original {
                        material_id: v.material.id.clone(),
                    },
                    observation_id,
                }],
            )
            .await?;
        }
        for v in &package.observations {
            self.require_source_provenance(
                ctx,
                ns,
                subject,
                &[SourceReadRef::Observation { id: v.id.clone() }],
            )
            .await?;
        }
        for v in &package.exposures {
            self.require_source_requirements(
                ctx,
                ns,
                subject,
                &[SourceReadRef::Media {
                    reference: v.locator.clone(),
                    observation_id: Some(v.observation_id.clone()),
                }],
            )
            .await?;
        }
        for v in &package.memories {
            self.require_source_read(
                ctx,
                ns,
                subject,
                &RetrievalTarget::Cognition(CognitiveRef::memory(&v.id, v.revision)),
            )
            .await?;
        }
        for v in &package.cognition {
            self.require_source_read(ctx, ns, subject, &RetrievalTarget::Cognition(v.reference()))
                .await?;
        }
        for v in &package.source_regions {
            let observation_id = package
                .observations
                .iter()
                .find(|event| event.material_id == v.material_id)
                .map(|event| event.id.clone());
            self.require_source_requirements(
                ctx,
                ns,
                subject,
                &[SourceReadRef::Media {
                    reference: v.reference(),
                    observation_id,
                }],
            )
            .await?;
        }
        for v in &package.representations {
            self.require_source_read(
                ctx,
                ns,
                subject,
                &RetrievalTarget::Media(MediaReference::Representation {
                    reference: v.reference(),
                }),
            )
            .await?;
        }
        for v in &package.derived_regions {
            self.require_source_read(ctx, ns, subject, &RetrievalTarget::Media(v.reference()))
                .await?;
        }
        for v in &package.tombstones {
            let sources: Vec<_> = v.sources.iter().map(SourceReadRef::evidence).collect();
            if !sources.is_empty() {
                self.require_source_provenance(ctx, ns, subject, &sources)
                    .await?;
            }
        }
        for v in &package.vectors.associations {
            if let Some(target) = v.input.target.item() {
                self.require_source_read(ctx, ns, subject, target).await?;
            }
            if !v.input.read_requirements.is_empty() {
                self.require_source_requirements(ctx, ns, subject, &v.input.read_requirements)
                    .await?;
            }
        }
        for call in &package.calls {
            self.require_model_call_sources(ctx, call).await?;
        }
        Ok(())
    }
}

// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use crate::*;

pub struct VcpProjectionMaterial {
    pub authority_watermark: i64,
    pub space: EmbeddingSpaceSignature,
    pub producer: ProducerSignature,
    pub identities: VcpIdentityMap,
    pub edges: Vec<WaveEdgeEvidence>,
    pub documents: Vec<VcpProjectedDocument>,
}
pub struct VcpProjectedDocument {
    pub reference: CognitiveRef,
    pub representation_text: String,
    pub vector: Vec<f32>,
    pub concept_refs: Vec<CognitiveRef>,
    pub curve_order: VcpCurveOrder,
    pub evidence_roots: std::collections::BTreeSet<String>,
}

impl ServingService {
    /// Resolve model material for a disposable VCP serving generation using
    /// the same host-supplied provider contract as dense construction.
    pub async fn vcp_projection_material(
        &self,
        subject: SubjectId,
        snapshot: &nous_configuration::ConfigSnapshot,
    ) -> Result<VcpProjectionMaterial> {
        self.vcp_projection_material_in_view(
            subject,
            snapshot,
            None,
            &self.publisher.snapshot_for(subject),
        )
        .await
    }
    pub(crate) async fn vcp_projection_material_in_view(
        &self,
        subject: SubjectId,
        snapshot: &nous_configuration::ConfigSnapshot,
        view: Option<&HistoricalAuthoritySnapshot>,
        serving: &ServingSnapshot,
    ) -> Result<VcpProjectionMaterial> {
        let provider = self.embedding().ok_or_else(|| {
            Error::Unavailable("VCP serving build requires configured embedding material".into())
        })?;
        let space = provider.space();
        let producer = provider.producer();
        let budget = snapshot.get(crate::EPISODE_SYNOPSIS)?;
        let capabilities = self.projection_capabilities(subject).await?;
        let input = self
            .projection_input(subject, capabilities.memory, budget, view)
            .await?;
        let concepts = serving.concept.iter().find(|generation| {
            generation
                .space
                .as_ref()
                .is_some_and(|s| s.compatible_with(&space))
                && generation
                    .producer
                    .as_ref()
                    .is_some_and(|p| p.signature_hash == producer.signature_hash)
        });
        let mut documents = Vec::new();
        for document in self.documents(subject, input.sources, budget, view).await? {
            let concept_refs = document
                .tag_ids
                .iter()
                .map(|id| {
                    id.parse()
                        .map(|id| CognitiveRef::Tag(TagId(id)))
                        .map_err(|_| {
                            Error::Infrastructure(
                                "cognitive projection contains an invalid Tag identity".into(),
                            )
                        })
                })
                .collect::<Result<Vec<_>>>()?;
            let output = self
                .embed_document(
                    subject,
                    &document,
                    concepts.map(AsRef::as_ref),
                    provider.as_ref(),
                )
                .await?
                .ok_or_else(|| {
                    Error::Unavailable("VCP requires compatible shared concept material".into())
                })?;
            let evidence_roots = input
                .evidence_roots
                .get(&document.reference)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .collect();
            documents.push(VcpProjectedDocument {
                evidence_roots,
                reference: document.reference,
                representation_text: document.representation_text,
                vector: output.vector,
                concept_refs,
                curve_order: VcpCurveOrder::StableIdentity,
            });
        }
        let identities = VcpIdentityMap::new(
            input
                .topology
                .nodes
                .into_iter()
                .chain(documents.iter().map(|d| d.reference.clone()))
                .chain(
                    documents
                        .iter()
                        .flat_map(|d| d.concept_refs.iter().cloned()),
                ),
        );
        Ok(VcpProjectionMaterial {
            authority_watermark: input.authority_watermark,
            space,
            producer,
            identities,
            edges: input.topology.edges,
            documents,
        })
    }
}

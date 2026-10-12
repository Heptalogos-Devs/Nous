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
    pub(crate) async fn vcp_projection_material_in_view(
        &self,
        subject: SubjectId,
        snapshot: &nous_configuration::ConfigSnapshot,
        view: Option<&HistoricalAuthoritySnapshot>,
        serving: &ServingSnapshot,
        capabilities: ProjectionCapabilities,
    ) -> Result<VcpProjectionMaterial> {
        let provider = self.embedding().ok_or_else(|| {
            Error::Unavailable("VCP serving build requires configured embedding material".into())
        })?;
        let space = provider.space();
        let producer = provider.producer();
        let budget = snapshot.get(crate::EPISODE_SYNOPSIS)?;
        let input = self
            .vcp_projection_input(
                subject,
                capabilities.memory,
                budget,
                &format!("vcp:{}", space.space_hash),
                view,
            )
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
        let source_documents = self.documents(subject, input.sources, budget, view).await?;
        let expected = source_documents.len();
        let embedded = self
            .embed_documents(
                subject,
                &source_documents,
                concepts.map(AsRef::as_ref),
                provider.as_ref(),
            )
            .await?;
        if embedded.len() != expected {
            return Err(Error::Unavailable(
                "VCP requires compatible shared concept material".into(),
            ));
        }
        for (index, output) in embedded {
            let document = &source_documents[index];
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
            let evidence_roots = input
                .evidence_roots
                .get(&document.reference)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .collect();
            documents.push(VcpProjectedDocument {
                evidence_roots,
                reference: document.reference.clone(),
                representation_text: document.representation_text.clone(),
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
            authority_watermark: input
                .watermarks
                .desired("topology", &format!("vcp:{}", space.space_hash)),
            space,
            producer,
            identities,
            edges: input.topology.edges,
            documents,
        })
    }
}

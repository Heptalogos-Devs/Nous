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
    pub(crate) async fn vcp_projection_material(
        &self,
        subject: SubjectId,
        input: &mut crate::assets::build::TextDocuments,
        serving: &ServingSnapshot,
    ) -> Result<VcpProjectionMaterial> {
        let provider = self.embedding().ok_or_else(|| {
            Error::Unavailable("VCP serving build requires configured embedding material".into())
        })?;
        let space = provider.space();
        let producer = provider.producer();
        let embedded = self.embedded_documents(subject, input, serving).await?;
        let source_documents = &input.documents;
        let expected = source_documents.len();
        let (topology, evidence_roots) = input
            .vcp
            .take()
            .ok_or_else(|| Error::Infrastructure("VCP topology missing".into()))?;
        let mut documents = Vec::new();
        if embedded.len() != expected {
            return Err(Error::Unavailable(
                "VCP requires compatible shared concept material".into(),
            ));
        }
        for (index, output) in embedded.iter() {
            let document = &source_documents[*index];
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
            let evidence_roots = evidence_roots
                .get(&document.reference)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .collect();
            documents.push(VcpProjectedDocument {
                evidence_roots,
                reference: document.reference.clone(),
                representation_text: document.representation_text.clone(),
                vector: output.vector.clone(),
                concept_refs,
                curve_order: VcpCurveOrder::StableIdentity,
            });
        }
        let identities = VcpIdentityMap::new(
            topology
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
            edges: topology.edges,
            documents,
        })
    }
}

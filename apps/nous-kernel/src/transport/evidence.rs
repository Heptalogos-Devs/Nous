// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
use super::*;
use nous_core::{DerivedRepresentationId, OccurrenceId, Result, SourceRegionId};

impl KernelService {
    pub(super) async fn get_occurrence(&self, input: p::ObjectRequest) -> Result<p::Occurrence> {
        Ok(occurrence(
            self.0
                .material
                .occurrence(
                    SubjectId(id(&input.subject_id)?),
                    OccurrenceId(id(&input.id)?),
                )
                .await?,
        ))
    }
    pub(super) async fn get_source_region(
        &self,
        input: p::ObjectRequest,
    ) -> Result<p::SourceRegion> {
        let value = self
            .0
            .material
            .source_region(
                SubjectId(id(&input.subject_id)?),
                SourceRegionId(id(&input.id)?),
            )
            .await?;
        Ok(source_region(value))
    }
    pub(super) async fn get_derived_representation(
        &self,
        input: p::ObjectRequest,
    ) -> Result<p::DerivedRepresentation> {
        Ok(representation(
            self.0
                .material
                .derived_representation(
                    SubjectId(id(&input.subject_id)?),
                    DerivedRepresentationId(id(&input.id)?),
                )
                .await?,
        ))
    }
}

pub(super) fn occurrence(value: nous_material::ObservationOccurrence) -> p::Occurrence {
    p::Occurrence {
        occurrence_id: value.occurrence_id.0.to_string(),
        subject_id: value.subject_id.0.to_string(),
        artifact_id: value.artifact_id.map(|id| id.0.to_string()),
        source_class: value.source_class.as_str().into(),
        world_realized: value.world_realized.map(|source| p::WorldRealizedSource {
            world_id: source.world_id,
            event_id: source.event_id,
            event_revision: source.event_revision,
            producer_ref: source.producer_ref,
            generation_method: source.generation_method,
            realized_at: Some(timestamp(source.realized_at)),
            world_observed: Some(domain_stamp_proto(&source.world_observed)),
            shared_origin_refs: source.shared_origin_refs,
        }),
        external_object_ref: value
            .external_object_ref
            .map(|value| value.as_str().to_owned()),
        occurred_time: Some(domain_temporal_proto(
            &value.occurred_time_domain_ref,
            &value.occurred_time,
        )),
        observed_at: Some(timestamp(value.observed_at)),
        observed_stamp: Some(domain_stamp_proto(&value.observed_stamp)),
        recorded_at: Some(timestamp(value.created_at)),
        conversation_ref: value.conversation_ref,
        actor_entity_ref: value
            .actor_entity_ref
            .map(|value| value.as_str().to_owned()),
        context: to_object(value.context),
    }
}

pub(super) fn producer(value: nous_core::ProducerSignature) -> p::ProducerSignature {
    p::ProducerSignature {
        signature_hash: value.signature_hash,
        provider_class: value.provider_class,
        operation: enum_name(value.operation),
        implementation: value.implementation,
        model_identity: value.model_identity,
        model_revision: value.model_revision,
        output_schema_digest: value.output_schema_digest,
        preprocessing_identity: value.preprocessing_identity,
        preprocessing_revision: value.preprocessing_revision,
        config_digest: value.config_digest,
        model_role: value.model_role,
        model_profile: value.model_profile,
        execution_profile: value.execution_profile,
        inference_controls_digest: value.inference_controls_digest,
        role_policy_digest: value.role_policy_digest,
        prompt_id: value.prompt_id,
        prompt_digest: value.prompt_digest,
    }
}

pub(super) fn representation(
    value: nous_material::DerivedRepresentation,
) -> p::DerivedRepresentation {
    let signature = value.producer.signature_hash.clone();
    p::DerivedRepresentation {
        representation_id: value.derived_representation_id.0.to_string(),
        formed_stamp: value.formed_stamp.as_ref().map(domain_stamp_proto),
        recorded_at: Some(timestamp(value.created_at)),
        subject_id: value.subject_id.0.to_string(),
        inputs: value
            .inputs
            .into_iter()
            .map(|input| p::DerivationInput {
                ordinal: input.ordinal,
                reference: Some(to_ref(input.reference)),
                role: input.role,
            })
            .collect(),
        producer: Some(producer(value.producer)),
        quality: to_object(value.quality),
        supersedes: value.supersedes.map(|id| id.0.to_string()),
        strategy: value.strategy,
        kind: value.representation_kind.as_str().into(),
        producer_signature: signature,
        revision: value.revision,
        text: value.payload_text,
        structured_payload: value.payload_json.and_then(to_object),
        artifact_id: value.payload_artifact_id.map(|id| id.0.to_string()),
    }
}

pub(super) fn source_region(value: nous_material::SourceRegion) -> p::SourceRegion {
    p::SourceRegion {
        source_region_id: value.source_region_id.0.to_string(),
        artifact_id: value.artifact_id.0.to_string(),
        coordinate_kind: value.coordinate_kind,
        coordinate: to_object(value.coordinate),
        coordinate_hash: value.coordinate_hash,
        parent_source_region_id: value.parent_source_region_id.map(|id| id.0.to_string()),
    }
}

pub(super) fn derived_region(value: nous_material::DerivedRegion) -> p::DerivedRegion {
    p::DerivedRegion {
        formed_at: value.formed_stamp.as_ref().map(domain_stamp_proto),
        recorded_at: Some(timestamp(value.created_at)),
        derived_region_id: value.derived_region_id.0.to_string(),
        subject_id: value.subject_id.0.to_string(),
        representation_id: value.derived_representation_id.0.to_string(),
        coordinate_kind: value.coordinate_kind,
        coordinate: to_object(value.coordinate),
        coordinate_hash: value.coordinate_hash,
        parent_derived_region_id: value.parent_derived_region_id.map(|id| id.0.to_string()),
    }
}

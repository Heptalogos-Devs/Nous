// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

use super::*;

rpc_service! {
    p::concept_service_server::ConceptService {
        forward {
            create_tag(p::CreateTagRequest) -> p::Tag;
            get_tag(p::ObjectRequest) -> p::Tag;
            list_tags(p::ListRequest) -> p::ListTagsResponse;
            search_tags(p::SearchTagsRequest) -> p::ListTagsResponse;
            revise_tag(p::ReviseTagRequest) -> p::Tag;
            merge_tags(p::MergeTagsRequest) -> p::Tag;
            split_tag(p::SplitTagRequest) -> p::SplitTagResponse;
            create_association(p::CreateAssociationRequest) -> p::Association;
            revoke_association(p::RevokeAssociationRequest) -> ();
            get_neighborhood(p::NeighborhoodRequest) -> p::NeighborhoodResponse;
            create_cognitive_schema(p::CreateCognitiveSchemaRequest) -> p::CognitiveSchema;
            add_schema_evidence(p::AddSchemaEvidenceRequest) -> p::CognitiveSchema;
            revise_cognitive_schema(p::ReviseCognitiveSchemaRequest) -> p::CognitiveSchema;
            split_cognitive_schema(p::SplitCognitiveSchemaRequest) -> p::SplitCognitiveSchemaResponse;
            merge_cognitive_schemas(p::MergeCognitiveSchemasRequest) -> p::CognitiveSchema;
        }
        custom {}
    }
}

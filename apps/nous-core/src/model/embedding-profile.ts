// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { blake3 } from "@noble/hashes/blake3.js";
import { modelRoleGraph, type ModelConfiguration } from "./configuration.js";
import { canonicalDigest } from "../digest.js";
import { modelRoleIdentity } from "./identity.js";
import { modelImplementations } from "./implementation.js";
const digest = (value: unknown) =>
  Buffer.from(blake3(new TextEncoder().encode(JSON.stringify(value)))).toString(
    "hex",
  );

export function resolvedEmbeddingRoute(
  config: ModelConfiguration,
  executionName: string,
) {
  const policy = config.roles.query_embedding;
  const configured = config.execution_profiles[executionName];
  const profile = configured && config.model_profiles[configured.model];
  const gateway = profile && config.gateway_profiles[profile.gateway];
  if (
    !policy?.routes.includes(executionName) ||
    !profile?.embedding ||
    !profile.model.trim() ||
    !gateway
  )
    return undefined;
  const binding = configured!;
  const identity = modelRoleIdentity(profile, gateway, binding);
  const policyDigest = canonicalDigest(policy);
  const snapshot = canonicalDigest({
    policy,
    configuration: modelRoleGraph(config, "query_embedding"),
  });
  const configDigest = canonicalDigest({
    snapshot,
    execution: identity.configDigest,
    policyDigest,
    executionName,
  });
  const space = {
    space_hash: "",
    model_identity: profile.model,
    weights_revision: profile.embedding.weights_revision,
    task: profile.embedding.task,
    input_representation: profile.embedding.input_representation,
    preprocessing_identity: profile.embedding.preprocessing_identity,
    preprocessing_revision: profile.embedding.preprocessing_revision,
    dimension: profile.embedding.dimension,
    normalization: profile.embedding.normalization,
    output_semantics: profile.embedding.output_semantics,
  };
  space.space_hash = digest(space);
  const producer = {
    signature_hash: "",
    provider_class: profile.protocol,
    operation: "text_embedding",
    implementation: `ai-sdk:${modelImplementations.provider}`,
    model_identity: profile.model,
    model_revision: profile.model_revision ?? null,
    output_schema_digest: null,
    preprocessing_identity: profile.embedding.preprocessing_identity,
    preprocessing_revision: profile.embedding.preprocessing_revision,
    config_digest: configDigest,
    model_role: "query_embedding",
    model_profile: binding.model,
    execution_profile: executionName,
    inference_controls_digest: canonicalDigest(binding),
    role_policy_digest: policyDigest,
    prompt_id: null,
    prompt_digest: null,
  };
  const {
    prompt_id: _promptId,
    prompt_digest: _promptDigest,
    ...signature
  } = producer;
  producer.signature_hash = digest(signature);
  return { space, producer };
}
export function resolvedEmbedding(config: ModelConfiguration) {
  const routes =
    config.roles.query_embedding?.routes
      .map((name) => resolvedEmbeddingRoute(config, name))
      .filter((value) => value !== undefined) ?? [];
  if (!routes.length) return undefined;
  if (
    routes.some(
      (route) => route.space.space_hash !== routes[0]!.space.space_hash,
    )
  )
    throw new Error("Embedding routes require identical full space signatures");
  return {
    space: routes[0]!.space,
    producers: routes.map((route) => route.producer),
  };
}

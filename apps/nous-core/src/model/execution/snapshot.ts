// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { canonicalDigest } from "../../digest.js";
import { type ModelRole } from "../roles.js";
import {
  type ModelExecutionConfiguration,
  modelRoleProblem,
  type RolePolicy,
} from "../configuration.js";
import { type ModelProfile, type ExecutionProfile } from "../profiles.js";
import { type PromptAsset } from "../prompts.js";
import { modelRoleIdentity } from "../identity.js";
import { providerContractForRole } from "../schemas/contracts.js";

export type ModelProducerMetadata = {
  implementation: string;
  protocol: string;
  model: string;
  modelRevision?: string;
  profileDigest: string;
  promptId?: string;
  promptDigest?: string;
  outputSchemaDigest?: string;
  configDigest: string;
  modelRole: ModelRole;
  modelProfile: string;
  executionProfile: string;
  inferenceControlsDigest: string;
  rolePolicyDigest: string;
};
export type ReadyRole = {
  name: ModelRole;
  profile: ModelProfile;
  binding: ExecutionProfile;
  executionName: string;
  policy: RolePolicy;
  policyDigest: string;
  prompt?: PromptAsset;
  baseURL: string;
  credential: string;
  credentialEnv: string;
  timeout: number;
  profileDigest: string;
  configDigest: string;
};
export type ModelRoleSnapshot = {
  configuration: ModelExecutionConfiguration;
  role: ModelRole;
  prompt?: PromptAsset;
  configDigest: string;
};

/** One binding owner for startup admission and frozen execution recovery. */
export function resolveRole(
  config: ModelRoleSnapshot["configuration"],
  name: ModelRole,
  executionName: string,
  prompt?: PromptAsset,
  snapshotDigest?: string,
): ReadyRole {
  const policy = config.roles[name];
  const binding = config.execution_profiles[executionName];
  const profile = binding && config.model_profiles[binding.model];
  if (!policy || !binding || !profile || !profile.model.trim())
    throw new Error("Execution/model profile is absent or unset");
  const mismatch = modelRoleProblem(name, binding, profile);
  if (mismatch) throw new Error(mismatch);
  const gateway = config.gateway_profiles[profile.gateway];
  const credential = gateway && process.env[gateway.credential_env];
  if (!gateway?.enabled || !credential)
    throw new Error("Gateway disabled or credential unavailable");
  if (!prompt && providerContractForRole(name))
    throw new Error("Prompt asset unavailable or invalid");
  const identity = modelRoleIdentity(profile, gateway, binding, prompt);
  const policyDigest = canonicalDigest(policy);
  return {
    name,
    profile,
    binding,
    executionName,
    policy,
    policyDigest,
    prompt,
    credential,
    credentialEnv: gateway.credential_env,
    baseURL: gateway.base_url,
    timeout: binding.timeout_ms ?? gateway.request_timeout_ms,
    profileDigest: identity.profileDigest,
    configDigest: canonicalDigest({
      ...(snapshotDigest && { snapshot: snapshotDigest }),
      execution: identity.configDigest,
      policyDigest,
      executionName,
    }),
  };
}

export function hydrateRole(
  snapshot: ModelRoleSnapshot,
  executionName: string,
  credentialOrigins: ReadonlySet<string>,
): ReadyRole {
  const role = resolveRole(
    snapshot.configuration,
    snapshot.role,
    executionName,
    snapshot.prompt,
    snapshot.configDigest,
  );
  if (
    !credentialOrigins.has(
      role.credentialEnv + "@" + new URL(role.baseURL).origin,
    )
  )
    throw new Error(
      "Reserved gateway credential destination is no longer authorized by current configuration",
    );
  return role;
}

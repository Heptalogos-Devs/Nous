// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { parseArgs } from "node:util";
import { mkdir, readFile, writeFile, rm } from "node:fs/promises";
import { dirname, relative, resolve, sep } from "node:path";
import { coreConfigurationValues } from "../../apps/nous-core/src/configuration/catalog.js";
import {
  parseConfiguration,
  parseEffectiveConfiguration,
} from "../../apps/nous-core/src/configuration/schema.js";
import {
  roleNames,
  type ModelRole,
} from "../../apps/nous-core/src/model/roles.js";
import { PromptRegistry } from "../../apps/nous-core/src/model/prompts.js";
import { providerContractForRole } from "../../apps/nous-core/src/model/schemas/contracts.js";
import {
  modelRoleIdentity,
  invocationConfigDigest,
} from "../../apps/nous-core/src/model/identity.js";

const { values } = parseArgs({
  options: {
    all: { type: "boolean" },
    role: { type: "string" },
    config: { type: "string" },
    output: { type: "string" },
    "prompt-root": { type: "string", default: "prompts" },
    "prompt-path": { type: "string" },
    adapter: { type: "string" },
    "override-prompt-root": { type: "string" },
  },
});
if (values["prompt-path"] && !values.role)
  throw new Error("--prompt-path requires --role");
if (values.role && !roleNames.includes(values.role as ModelRole))
  throw new Error("Unknown model role");
if (!values.all && !values.role)
  throw new Error("Select --all or --role <role>");
let configuration;
if (values.config) {
  const { document } = parseConfiguration(
    await readFile(resolve(values.config), "utf8"),
    true,
  );
  configuration = parseEffectiveConfiguration(
    coreConfigurationValues(document),
  ).models;
}
const prompts = new PromptRegistry(
  resolve(values["prompt-root"]!),
  values["override-prompt-root"]
    ? resolve(values["override-prompt-root"])
    : values.config
      ? resolve(dirname(values.config), "prompts")
      : undefined,
);
const output = resolve(
  values.output ?? "data/research/inspection/model-contracts",
);
const rel = relative(resolve("data/research"), output);
if (rel === ".." || rel.startsWith(`..${sep}`) || rel.startsWith(sep))
  throw new Error("Inspection output must be under ignored data/research");
for (const role of values.role ? [values.role as ModelRole] : roleNames) {
  const contract = providerContractForRole(role);
  const configured = configuration?.roles[role];
  const executionName = configured?.routes[0];
  const execution = executionName
    ? configuration?.execution_profiles[executionName]
    : undefined;
  const profile = execution
    ? configuration?.model_profiles[execution.model]
    : undefined;
  const gateway = profile
    ? configuration?.gateway_profiles[profile.gateway]
    : undefined;
  const binding = execution ? execution : undefined;
  const basePrompt = await prompts.load(role, configured?.prompt);
  const prompt = values["prompt-path"]
    ? await prompts.load(role, values["prompt-path"])
    : basePrompt;
  const identity =
    profile && gateway && binding
      ? modelRoleIdentity(profile, gateway, binding, basePrompt)
      : undefined;
  if (identity)
    identity.configDigest = invocationConfigDigest(
      identity.configDigest,
      values["prompt-path"] ? prompt : undefined,
      values.adapter,
    );
  const metadata = {
    role,
    contract_id: contract?.id ?? null,
    provider_name: contract?.providerName ?? null,
    schema_digest: contract?.digest ?? null,
    prompt_id: prompt?.id ?? null,
    prompt_digest: prompt?.digest ?? null,
    ...(profile
      ? {
          model_profile: execution!.model,
          model: profile.model,
          protocol: profile.protocol,
          model_revision: profile.model_revision ?? null,
        }
      : {}),
    ...(identity
      ? {
          profile_digest: identity.profileDigest,
          config_digest: identity.configDigest,
        }
      : {}),
  };
  const root = values.role ? output : resolve(output, role);
  await mkdir(root, { recursive: true, mode: 0o700 });
  await writeFile(
    resolve(root, "contract.json"),
    JSON.stringify(metadata, null, 2) + "\n",
    { mode: 0o600 },
  );
  if (contract)
    await writeFile(
      resolve(root, "schema.json"),
      JSON.stringify(contract.providerSchema, null, 2) + "\n",
      { mode: 0o600 },
    );
  else await rm(resolve(root, "schema.json"), { force: true });
  if (prompt)
    await writeFile(resolve(root, "prompt.md"), prompt.text, { mode: 0o600 });
  else await rm(resolve(root, "prompt.md"), { force: true });
  console.log(JSON.stringify({ ...metadata, output: root }));
}

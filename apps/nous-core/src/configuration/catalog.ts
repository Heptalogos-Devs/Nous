// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { z } from "zod";
import { consumerStatePolicySchema } from "@nous-wave/client/consumer-policy";
import { executionOpportunitySchema } from "@nous-wave/client/execution-policy";
import {
  modelConfigurationShape,
  modelExecutionSchema,
} from "../model/configuration.js";

export const CONFIG_REVISION = 2;
const requirement = z.enum(["REQUIRED", "PREFERRED", "OPTIONAL", "FORBIDDEN"]);
export const consumersSchema = z
  .array(
    z.strictObject({
      consumer_id: z.string().min(1).max(128),
      revision: z.string().min(1).default("1"),
      memory: requirement.default("PREFERRED"),
      runtime: requirement.default("OPTIONAL"),
      resource: requirement.default("OPTIONAL"),
      max_items: z.number().int().min(1).max(2048).default(32),
      max_text_bytes: z.number().int().min(1).max(1048576).default(32768),
      materialize: z.boolean().default(true),
    }),
  )
  .min(1)
  .max(64)
  .refine(
    (consumers) =>
      new Set(consumers.map((consumer) => consumer.consumer_id)).size ===
      consumers.length,
    "Duplicate consumer policy ID",
  )
  .prefault([{ consumer_id: "default" }]);
const executionTimeout = z.number().int().min(1).max(600000);
const hostFields = {
  port: z.number().int().min(0).max(65535).default(9470),
  dotenv_file: z.string().min(1).default("gateway.env"),
  kernel_executable: z.string().min(1).nullable().default(null),
  kernel_startup_timeout_ms: executionTimeout.default(120000),
  kernel_shutdown_timeout_ms: executionTimeout.default(5000),
  runtime_download_timeout_ms: executionTimeout.default(300000),
};
export const hostSchema = z.strictObject(hostFields).prefault({});
export const coreExecutionSchema = z
  .strictObject({
    kernel_rpc_timeout_ms: executionTimeout.default(30000),
    maintenance_rpc_timeout_ms: executionTimeout.default(10000),
    opportunity: executionOpportunitySchema,
    context_track_limit: z.number().int().min(1).max(65536).default(256),
    query_embedding_cache_entries: z
      .number()
      .int()
      .min(1)
      .max(65536)
      .default(128),
    query_rerank_candidate_limit: z.number().int().min(2).max(64).default(64),
    public_rpc_response_max_bytes: z
      .number()
      .int()
      .min(1024)
      .max(33554432)
      .default(4194304),
    http_body_limit_bytes: z
      .number()
      .int()
      .min(1024)
      .max(8388608)
      .default(2097152),
  })
  .prefault({});
export type CoreExecutionPolicy = z.infer<typeof coreExecutionSchema>;

const databaseSchema = z
  .strictObject({
    mode: z.enum(["managed_private", "external"]).default("managed_private"),
    url_env: z
      .string()
      .regex(/^[A-Za-z_][A-Za-z0-9_]*$/)
      .optional(),
    max_connections: z.number().int().min(1).max(256).default(8),
    acquire_timeout_ms: executionTimeout.default(15000),
    name: z
      .string()
      .regex(/^[a-z][a-z0-9_]{0,62}$/)
      .default("nous_wave"),
  })
  .prefault({});
export const bootstrapSchema = z.object({
  config_revision: z.literal(CONFIG_REVISION),
  host: hostSchema,
  database: databaseSchema,
});

const owners = [
  {
    path: "config_revision",
    schema: z.literal(CONFIG_REVISION),
    default: CONFIG_REVISION,
    owner: "core-host",
    exposure: "developer",
    deployment: true,
  },
  ...Object.entries(hostFields).map(([name, schema]) => ({
    path: `host.${name}`,
    schema,
    default: schema.parse(undefined),
    owner: "core-host",
    exposure:
      name === "port" || name === "dotenv_file" ? "standard" : "developer",
    deployment: true,
  })),
  ...fields(
    "core_execution",
    coreExecutionSchema.unwrap().shape,
    "core-execution",
    "developer",
  ),
  ...fields(
    "core_execution.opportunity",
    executionOpportunitySchema.unwrap().shape,
    "core-execution",
    "developer",
  ),
  {
    path: "database",
    schema: databaseSchema,
    default: databaseSchema.parse(undefined),
    owner: "core-host",
    exposure: "advanced",
    deployment: true,
  },
  {
    path: "models",
    schema: modelExecutionSchema,
    default: modelExecutionSchema.parse(undefined),
    owner: "core-model",
    exposure: "standard",
    effect: "authority_formation",
  },
  ...fields(
    "audio",
    modelConfigurationShape.audio.unwrap().shape,
    "core-material",
    "advanced",
    "authority_formation",
  ),
  ...fields(
    "video",
    modelConfigurationShape.video.unwrap().shape,
    "core-material",
    "advanced",
    "authority_formation",
  ),
  {
    path: "material.strategy",
    schema: modelConfigurationShape.material_strategy,
    default: "description_only",
    owner: "core-material",
    exposure: "standard",
    effect: "authority_formation",
  },
  ...fields(
    "material.inputs",
    modelConfigurationShape.material_inputs.unwrap().shape,
    "core-material",
    "developer",
    "authority_formation",
  ),
  {
    path: "consumers",
    schema: consumersSchema,
    default: consumersSchema.parse(undefined),
    owner: "core-consumer",
    exposure: "advanced",
    effect: "query_policy",
  },
  ...fields(
    "consumer_state",
    consumerStatePolicySchema.unwrap().shape,
    "official-consumer",
    "advanced",
  ),
] satisfies {
  path: string;
  schema: z.ZodType;
  default: unknown;
  owner: string;
  exposure: string;
  deployment?: boolean;
  effect?: string;
  unit?: string;
}[];

/** Publish independent fields from their actual owner types; the model graph stays atomic. */
function fields(
  prefix: string,
  shape: Record<string, z.ZodType>,
  owner: string,
  exposure: string,
  effect = "operational",
) {
  return Object.entries(shape)
    .filter(
      ([name]) => !(prefix === "core_execution" && name === "opportunity"),
    )
    .map(([name, schema]) => ({
      path: `${prefix}.${name}`,
      schema,
      default: schema.parse(undefined),
      owner,
      exposure,
      effect:
        prefix === "core_execution" &&
        ["query_rerank_candidate_limit"].includes(name)
          ? "query_policy"
          : effect,
      unit: name.endsWith("_ms")
        ? "milliseconds"
        : name.endsWith("_seconds")
          ? "seconds"
          : name.endsWith("_bytes")
            ? "bytes"
            : undefined,
    }));
}

/** Read deployment values through the same registered paths for offline owner inspection. */
export function coreConfigurationValues(document: Record<string, unknown>) {
  return Object.fromEntries(
    owners.map((owner) => {
      let value: unknown = document;
      for (const part of owner.path.split("."))
        value =
          value && typeof value === "object" && !Array.isArray(value)
            ? (value as Record<string, unknown>)[part]
            : undefined;
      return [owner.path, owner.schema.parse(value)];
    }),
  );
}

/** Reassemble one typed group from Kernel-resolved leaf values. */
export function coreConfigurationGroup(
  values: Record<string, unknown>,
  prefix: string,
) {
  const group: Record<string, unknown> = {};
  for (const owner of owners) {
    if (!owner.path.startsWith(`${prefix}.`)) continue;
    const parts = owner.path.slice(prefix.length + 1).split(".");
    let target = group;
    for (const part of parts.slice(0, -1))
      target = (target[part] ??= {}) as Record<string, unknown>;
    target[parts.at(-1)!] = values[owner.path];
  }
  return group;
}

/** Owner Zod schemas are the sole TypeScript type/JSON Schema source. */
function coreDescriptors() {
  return owners.map((owner) => ({
    path: owner.path,
    owner: owner.owner,
    title: owner.path,
    description: `Configuration for ${owner.path}.`,
    category: owner.owner,
    json_schema: z.toJSONSchema(owner.schema, {
      target: "draft-2020-12",
      io: "output",
    }),
    reference_default: owner.default,
    exposure: owner.exposure,
    scope_policy: "system_only",
    storage_policy: "deployment" in owner ? "deployment_only" : "overrideable",
    apply_mode: "restart_process",
    semantic_effect: "effect" in owner ? owner.effect : "operational",
    unit: "unit" in owner ? (owner.unit ?? null) : null,
    sensitivity: ["models", "database"].includes(owner.path)
      ? "credential_reference"
      : "normal",
    reference_profile: null,
  }));
}
export function configurationBundle(deploymentDocument: unknown) {
  const document = structuredClone(deploymentDocument) as Record<
    string,
    unknown
  >;
  for (const owner of owners) {
    const parts = owner.path.split(".");
    let group = document;
    for (const part of parts.slice(0, -1)) {
      const child = group[part];
      if (!child || typeof child !== "object" || Array.isArray(child)) {
        group = {};
        break;
      }
      group = child as Record<string, unknown>;
    }
    const key = parts.at(-1)!;
    if (Object.hasOwn(group, key)) group[key] = owner.schema.parse(group[key]);
  }
  return {
    core_descriptors: coreDescriptors(),
    deployment_document: document,
  };
}

/** Core-owned configuration crosses into Kernel only after its Zod owner normalizes it. */
export function normalizeCoreConfigurationValue(path: string, value: unknown) {
  const owner = owners.find((candidate) => candidate.path === path);
  return owner ? owner.schema.parse(value) : value;
}

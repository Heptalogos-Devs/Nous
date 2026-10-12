// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import {
  create,
  fromJson,
  toJson,
  type DescMessage,
  type MessageShape,
  type JsonValue,
} from "@bufbuild/protobuf";
import { RevisionBasisSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { referenceSlots } from "@nous-wave/protocol/messages";
import { boundedInteger, CliError } from "./agent.js";
import { revisionBasis } from "./mutation-input.js";
import type { CliEnvironment } from "./runtime.js";

export const operationId = (env: CliEnvironment) =>
  env.values["operation-id"] ?? crypto.randomUUID();
export const page = (env: CliEnvironment) => ({
  pageSize: boundedInteger(
    env.values["page-size"] ?? "50",
    1,
    200,
    "--page-size",
  ),
  pageToken: env.values["page-token"],
});
export async function target(
  env: CliEnvironment,
  ref: string | undefined,
  kind: string,
) {
  return (
    await env.resolveReference(env.required(ref, `${kind} reference`), kind)
  ).value;
}
/** Semantic TOML content uses the owner's declared fields. Resolve references once before freezing. */
export async function content<S extends DescMessage>(
  env: CliEnvironment,
  schema: S,
  payload?: unknown,
): Promise<MessageShape<S>> {
  async function bind(
    value: unknown,
    declared: DescMessage,
  ): Promise<JsonValue> {
    if (
      declared.typeName === "nous.wave.v1alpha1.CognitiveRef" &&
      typeof value === "string"
    )
      return await env.resolveReference(value);
    if (
      declared.typeName === RevisionBasisSchema.typeName &&
      value &&
      typeof value === "object" &&
      "ref" in value
    ) {
      const basis = await revisionBasis(
        env,
        value as Parameters<typeof revisionBasis>[1],
      );
      return toJson(RevisionBasisSchema, create(RevisionBasisSchema, basis));
    }
    if (value instanceof Date) return value.toISOString();
    if (
      !value ||
      typeof value !== "object" ||
      declared.typeName.startsWith("google.protobuf.")
    )
      return value as JsonValue;
    const object = value as Record<string, unknown>;
    const result: Record<string, JsonValue> = {};
    for (const [key, member] of Object.entries(object)) {
      const field = declared.fields.find(
        (candidate) => candidate.name === key || candidate.jsonName === key,
      );
      if (!field)
        throw new CliError(
          "INVALID_ARGUMENT",
          `Unknown ${declared.name} field: ${key}`,
        );
      result[field.jsonName] =
        field.fieldKind === "message"
          ? await bind(member, field.message)
          : field.fieldKind === "list" &&
              field.listKind === "message" &&
              Array.isArray(member)
            ? await Promise.all(member.map((item) => bind(item, field.message)))
            : (member as JsonValue);
    }
    return result;
  }
  const message = fromJson(
    schema,
    await bind(
      payload ?? (await env.requestPayload(env.values["request-file"])),
      schema,
    ),
  );
  const resolved = new Map<string, Promise<{ kind: string; value: string }>>();
  for (const slot of referenceSlots(message, schema)) {
    const key = `${slot.kind}:${slot.value}`;
    let reference = resolved.get(key);
    if (!reference) {
      reference = env.resolveReference(slot.value, slot.kind);
      resolved.set(key, reference);
    }
    slot.parent[slot.key] = (await reference).value;
  }
  return message;
}

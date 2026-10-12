// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import {
  createRegistry,
  isMessage,
  toJson,
  type DescMessage,
  type DescFile,
  getOption,
  type DescField,
} from "@bufbuild/protobuf";
import {
  file_nous_wave_v1alpha1_types,
  CognitiveRefSchema,
  AcceptedObservationSchema,
  ArtifactSchema,
  WorkContextSchema,
  SessionSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { reference_kind } from "@nous-wave/protocol/nous/wave/v1alpha1/presentation_pb.js";
import { file_nous_wave_v1alpha1_journal } from "@nous-wave/protocol/nous/wave/v1alpha1/journal_pb.js";
import { file_nous_wave_v1alpha1_identity } from "@nous-wave/protocol/nous/wave/v1alpha1/identity_pb.js";
import { file_nous_wave_v1alpha1_errors } from "@nous-wave/protocol/nous/wave/v1alpha1/errors_pb.js";
import { file_nous_wave_v1alpha1_configuration } from "@nous-wave/protocol/nous/wave/v1alpha1/configuration_pb.js";
import { file_nous_wave_v1alpha1_management } from "@nous-wave/protocol/nous/wave/v1alpha1/management_pb.js";
import { file_nous_wave_v1alpha1_model } from "@nous-wave/protocol/nous/wave/v1alpha1/model_pb.js";
import { file_nous_wave_v1alpha1_services } from "@nous-wave/protocol/nous/wave/v1alpha1/services_pb.js";

const roots = [
  file_nous_wave_v1alpha1_types,
  file_nous_wave_v1alpha1_journal,
  file_nous_wave_v1alpha1_identity,
  file_nous_wave_v1alpha1_errors,
  file_nous_wave_v1alpha1_configuration,
  file_nous_wave_v1alpha1_management,
  file_nous_wave_v1alpha1_model,
  file_nous_wave_v1alpha1_services,
];
const files = new Set<DescFile>();
function registerFile(file: DescFile) {
  if (files.has(file)) return;
  files.add(file);
  file.dependencies.forEach(registerFile);
}
roots.forEach(registerFile);
const registry = createRegistry(...files);
// Keep schemas off the message itself and out of opaque user JSON.
const schemas = new WeakMap<object, DescMessage>();
export const clientDataSchemas = {
  CognitiveRefSchema,
  AcceptedObservationSchema,
  ArtifactSchema,
  WorkContextSchema,
  SessionSchema,
};
export function protocolReferenceKind(field: DescField) {
  return getOption(field, reference_kind);
}

/** Exact dependencies follow canonical field annotations; opaque user JSON is never traversed. */
export function protocolReferences(input: unknown, declared: DescMessage) {
  const references = new Map<string, { kind: string; value: string }>();
  const visit = (value: unknown, schema: DescMessage) => {
    if (!value || typeof value !== "object" || jsonTypes.has(schema.typeName))
      return;
    const object = value as Record<string, unknown>;
    for (const field of schema.fields) {
      const selected = field.oneof
        ? (object[field.oneof.localName] as
            { case?: string; value?: unknown } | undefined)
        : undefined;
      const member = field.oneof
        ? selected?.case === field.localName
          ? selected.value
          : undefined
        : object[field.localName];
      const declaredKind = protocolReferenceKind(field);
      const kind = declaredKind === "$kind" ? object.kind : declaredKind;
      const add = (candidate: unknown) => {
        if (
          typeof kind === "string" &&
          kind &&
          typeof candidate === "string" &&
          candidate
        )
          references.set(`${kind}:${candidate}`, { kind, value: candidate });
      };
      if (field.fieldKind === "message") visit(member, field.message);
      else if (field.fieldKind === "list" && Array.isArray(member)) {
        for (const child of member) {
          if (field.listKind === "message") visit(child, field.message);
          else add(child);
        }
      } else if (
        field.fieldKind === "map" &&
        member &&
        typeof member === "object"
      ) {
        for (const child of Object.values(member)) {
          if (field.mapKind === "message") visit(child, field.message);
          else add(child);
        }
      } else add(member);
    }
  };
  visit(input, declared);
  return [...references.values()];
}
const jsonTypes = new Set([
  "google.protobuf.Value",
  "google.protobuf.Struct",
  "google.protobuf.ListValue",
]);

/** Trusted Client output keeps its protocol schema out of user data and serialization. */
export function protocolSchema(value: unknown) {
  return value !== null && typeof value === "object"
    ? schemas.get(value)
    : undefined;
}

/** Attach trusted protocol schemas without inspecting JSON-shaped Struct leaves. */
export function restoreProtocolData<T>(value: T, schema: DescMessage): T {
  if (!value || typeof value !== "object") return value;
  if (jsonTypes.has(schema.typeName)) {
    if (
      schema.typeName !== "google.protobuf.Struct" &&
      isMessage(value, schema)
    )
      schemas.set(value, schema);
    return value;
  }
  schemas.set(value, schema);
  const object = value as Record<string, unknown>;
  for (const field of schema.fields) {
    const item = field.oneof
      ? (object[field.oneof.localName] as
          { case?: string; value?: unknown } | undefined)
      : undefined;
    const member = field.oneof
      ? item?.case === field.localName
        ? item.value
        : undefined
      : object[field.localName];
    if (field.fieldKind === "message") {
      if (field.message.typeName !== "google.protobuf.Struct")
        restoreProtocolData(member, field.message);
    } else if (
      field.fieldKind === "list" &&
      field.listKind === "message" &&
      Array.isArray(member)
    ) {
      if (field.message.typeName !== "google.protobuf.Struct")
        for (const child of member) restoreProtocolData(child, field.message);
    } else if (
      field.fieldKind === "map" &&
      field.mapKind === "message" &&
      member &&
      typeof member === "object" &&
      field.message.typeName !== "google.protobuf.Struct"
    )
      for (const child of Object.values(member))
        restoreProtocolData(child, field.message);
  }
  return value;
}

export function protocolSchemaByName(name: string) {
  return registry.getMessage(name);
}

/** Preserve the generated response and remember its canonical message schemas. */
export function protocolData<T>(value: T, declared?: DescMessage): T {
  if (!isMessage(value)) return value;
  const schema = declared ?? registry.getMessage(value.$typeName);
  if (!schema || schema.typeName !== value.$typeName) return value;
  if (jsonTypes.has(schema.typeName)) {
    schemas.set(value, schema);
    return value;
  }
  return restoreProtocolData(value, schema);
}

/** Expose the existing CLI shape without protobuf markers at its JSON boundary. */
export function protocolJson(value: unknown) {
  const schema = protocolSchema(value);
  if (!schema) return value;
  if (jsonTypes.has(schema.typeName)) return toJson(schema, value as never);
  const {
    $typeName: _typeName,
    $unknown: _unknown,
    ...json
  } = value as Record<string, unknown>;
  return json;
}

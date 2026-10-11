// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import {
  createRegistry,
  isMessage,
  toJson,
  type DescMessage,
  type DescFile,
  type JsonValue,
  getOption,
  type DescField,
} from "@bufbuild/protobuf";
import {
  ValueSchema,
  StructSchema,
  ListValueSchema,
} from "@bufbuild/protobuf/wkt";
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

export type Data<T> = T extends { $typeName: "google.protobuf.Value" }
  ? JsonValue
  : T extends { $typeName: "google.protobuf.Struct" }
    ? Record<string, JsonValue>
    : T extends { $typeName: "google.protobuf.ListValue" }
      ? JsonValue[]
      : T extends Uint8Array
        ? Uint8Array
        : T extends readonly (infer I)[]
          ? Data<I>[]
          : T extends object
            ? {
                [
                  K in keyof T as K extends "$typeName" | "$unknown" ? never : K
                ]: Data<T[K]>;
              }
            : T;

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

/** Restore known schema structure after a consumer receipt roundtrip; JSON leaves stay opaque. */
export function restoreProtocolData<T>(value: T, schema: DescMessage): T {
  if (!value || typeof value !== "object" || jsonTypes.has(schema.typeName))
    return value;
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
    if (field.fieldKind === "message")
      restoreProtocolData(member, field.message);
    else if (
      field.fieldKind === "list" &&
      field.listKind === "message" &&
      Array.isArray(member)
    )
      for (const child of member) restoreProtocolData(child, field.message);
    else if (
      field.fieldKind === "map" &&
      field.mapKind === "message" &&
      member &&
      typeof member === "object"
    )
      for (const child of Object.values(member))
        restoreProtocolData(child, field.message);
  }
  return value;
}

export function protocolSchemaByName(name: string) {
  return registry.getMessage(name);
}

/** Decode protobuf JSON at its owning type, never inspect arbitrary JSON for protocol keys. */
export function protocolData<T>(value: T, declared?: DescMessage): Data<T> {
  // Buf represents Struct fields as JsonObject; a user $typeName is never a message marker there.
  if (declared?.typeName === "google.protobuf.Struct") return value as Data<T>;
  if (isMessage(value, ValueSchema))
    return toJson(ValueSchema, value) as Data<T>;
  if (isMessage(value, StructSchema))
    return toJson(StructSchema, value) as Data<T>;
  if (isMessage(value, ListValueSchema))
    return toJson(ListValueSchema, value) as Data<T>;
  if (declared && jsonTypes.has(declared.typeName)) return value as Data<T>;
  if (value instanceof Uint8Array) return value as Data<T>;
  if (Array.isArray(value))
    return (value as unknown[]).map((member) =>
      protocolData(member),
    ) as Data<T>;
  if (value !== null && typeof value === "object") {
    const message = isMessage(value);
    const schema =
      declared ?? (message ? registry.getMessage(value.$typeName) : undefined);
    const result = Object.fromEntries(
      Object.entries(value)
        .filter(
          ([key]) => !(message && (key === "$typeName" || key === "$unknown")),
        )
        .map(([key, member]) => {
          const oneof = schema?.oneofs.find((group) => group.localName === key);
          if (oneof && member && typeof member === "object") {
            const selected = member as { case?: string; value?: unknown };
            const field = oneof.fields.find(
              (candidate) => candidate.localName === selected.case,
            );
            return [
              key,
              field?.fieldKind === "message"
                ? {
                    ...selected,
                    value: protocolData(selected.value, field.message),
                  }
                : member,
            ];
          }
          const field = schema?.fields.find(
            (candidate) => candidate.localName === key,
          );
          if (field?.fieldKind === "message")
            return [key, protocolData(member, field.message)];
          if (
            field?.fieldKind === "list" &&
            field.listKind === "message" &&
            Array.isArray(member)
          )
            return [
              key,
              (member as unknown[]).map((child) =>
                protocolData(child, field.message),
              ),
            ];
          if (
            field?.fieldKind === "map" &&
            field.mapKind === "message" &&
            member &&
            typeof member === "object"
          )
            return [
              key,
              Object.fromEntries(
                Object.entries(member as Record<string, unknown>).map(
                  ([name, child]) => [name, protocolData(child, field.message)],
                ),
              ),
            ];
          return [key, member];
        }),
    );
    if (schema) schemas.set(result, schema);
    return result as Data<T>;
  }
  return value as Data<T>;
}

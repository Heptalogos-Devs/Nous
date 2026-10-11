// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import {
  createRegistry,
  isMessage,
  type DescFile,
  type DescMessage,
  type DescField,
  getOption,
} from "@bufbuild/protobuf";
import { file_nous_wave_v1alpha1_types } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
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

export function protocolReferenceKind(field: DescField) {
  return getOption(field, reference_kind);
}

/** Exact dependencies follow canonical field annotations; opaque user JSON is never traversed. */
export function protocolReferences(input: unknown, declared: DescMessage) {
  if (!isMessage(input, declared)) return [];
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

/** Resolve the descriptor of a real Buf message without inferring from plain objects. */
export function protocolMessageSchema(value: unknown): DescMessage | undefined {
  if (!isMessage(value)) return undefined;
  return registry.getMessage(value.$typeName);
}

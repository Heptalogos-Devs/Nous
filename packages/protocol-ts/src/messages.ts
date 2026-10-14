// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import {
  createRegistry,
  toJson,
  type Message,
  isMessage,
  type DescMessage,
  type DescFile,
  getOption,
  type DescField,
} from "@bufbuild/protobuf";
import { file_nous_wave_v1alpha1_types } from "./generated/nous/wave/v1alpha1/types_pb.js";
import { reference_kind } from "./generated/nous/wave/v1alpha1/presentation_pb.js";
import { file_nous_wave_v1alpha1_journal } from "./generated/nous/wave/v1alpha1/journal_pb.js";
import { file_nous_wave_v1alpha1_identity } from "./generated/nous/wave/v1alpha1/identity_pb.js";
import { file_nous_wave_v1alpha1_errors } from "./generated/nous/wave/v1alpha1/errors_pb.js";
import { file_nous_wave_v1alpha1_configuration } from "./generated/nous/wave/v1alpha1/configuration_pb.js";
import { file_nous_wave_v1alpha1_management } from "./generated/nous/wave/v1alpha1/management_pb.js";
import { file_nous_wave_v1alpha1_model } from "./generated/nous/wave/v1alpha1/model_pb.js";
import { file_nous_wave_v1alpha1_services } from "./generated/nous/wave/v1alpha1/services_pb.js";

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

/** Native reference slots are shared by input binding, dependency collection and presentation. */
export function referenceSlots(
  input: unknown,
  declared?: DescMessage,
  scope = "",
) {
  const slots: {
    parent: Record<string | number, unknown>;
    key: string | number;
    kind: string;
    value: string;
    subjectId: string;
  }[] = [];
  function visit(
    value: unknown,
    schema = protocolSchema(value),
    subjectId = scope,
  ) {
    if (
      !value ||
      typeof value !== "object" ||
      value instanceof Uint8Array ||
      (schema && jsonTypes.has(schema.typeName))
    )
      return;
    if (Array.isArray(value)) {
      value.forEach((member) => visit(member, schema, subjectId));
      return;
    }
    const object = value as Record<string, unknown>;
    if (!schema) {
      Object.values(object).forEach((member) =>
        visit(member, undefined, subjectId),
      );
      return;
    }
    const subject = schema.fields.find(
      (field) => protocolReferenceKind(field) === "subject",
    );
    if (
      subject &&
      typeof object[subject.localName] === "string" &&
      object[subject.localName]
    )
      subjectId = object[subject.localName] as string;
    for (const field of schema.fields) {
      const parent = field.oneof
        ? (object[field.oneof.localName] as
            { case?: string; value?: unknown } | undefined)
        : object;
      if (!parent || (field.oneof && parent.case !== field.localName)) continue;
      const key = field.oneof ? "value" : field.localName;
      const member = (parent as Record<string, unknown>)[key];
      const declaredKind = protocolReferenceKind(field);
      const kind = declaredKind === "$kind" ? object.kind : declaredKind;
      const add = (
        container: object,
        addressKey: string | number,
        candidate: unknown,
      ) => {
        if (
          typeof kind === "string" &&
          kind &&
          typeof candidate === "string" &&
          candidate
        )
          slots.push({
            parent: container as Record<string | number, unknown>,
            key: addressKey,
            kind,
            value: candidate,
            subjectId: kind === "subject" ? candidate : subjectId,
          });
      };
      if (field.fieldKind === "message")
        visit(member, field.message, subjectId);
      else if (field.fieldKind === "list" && Array.isArray(member))
        member.forEach((child, index) =>
          field.listKind === "message"
            ? visit(child, field.message, subjectId)
            : add(member, index, child),
        );
      else if (
        field.fieldKind === "map" &&
        member &&
        typeof member === "object"
      )
        Object.entries(member).forEach(([name, child]) =>
          field.mapKind === "message"
            ? visit(child, field.message, subjectId)
            : add(member, name, child),
        );
      else add(parent, key, member);
    }
  }
  visit(input, declared);
  return slots;
}

/** Exact dependencies follow canonical annotations; freeform JSON remains opaque. */
export function protocolReferences(input: unknown, declared: DescMessage) {
  return [
    ...new Map(
      referenceSlots(input, declared).map(({ kind, value }) => [
        `${kind}:${value}`,
        { kind, value },
      ]),
    ).values(),
  ];
}
const jsonTypes = new Set([
  "google.protobuf.Value",
  "google.protobuf.Struct",
  "google.protobuf.ListValue",
]);

/** Canonical messages carry their declared type; no parallel DTO registry is needed. */
export function protocolSchema(value: unknown) {
  return isMessage(value) ? registry.getMessage(value.$typeName) : undefined;
}

/** Buf owns native JSON encoding; recurse only through consumer-owned command envelopes. */
export function messageData(value: unknown): unknown {
  if (!value || typeof value !== "object") return value;
  if (Array.isArray(value)) return value.map(messageData);
  if (value instanceof Uint8Array) return value;
  const schema = protocolSchema(value);
  if (schema) {
    const json = toJson(schema, value as Message, { alwaysEmitImplicit: true });
    const extra = Object.entries(value).filter(
      ([key]) =>
        key !== "$typeName" &&
        key !== "$unknown" &&
        !schema.fields.some((field) => field.localName === key) &&
        !schema.oneofs.some((oneof) => oneof.localName === key),
    );
    return extra.length &&
      json &&
      typeof json === "object" &&
      !Array.isArray(json)
      ? {
          ...json,
          ...Object.fromEntries(
            extra.map(([key, item]) => [key, messageData(item)]),
          ),
        }
      : json;
  }
  return Object.fromEntries(
    Object.entries(value).map(([key, item]) => [key, messageData(item)]),
  );
}

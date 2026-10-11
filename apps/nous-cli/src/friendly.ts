// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { toJson, type Message } from "@bufbuild/protobuf";
import {
  protocolMessageSchema,
  protocolReferenceKind,
} from "@nous-wave/client/data";
import type { CliEnvironment } from "./runtime.js";

/** Add lexical references as display metadata without replacing canonical values. */
export async function friendlyOutput(
  input: unknown,
  env: CliEnvironment,
): Promise<unknown> {
  type Target = {
    subjectId: string;
    canonical: { kind: string; value: string };
  };
  const targets = new Map<string, Target>();
  const slots: {
    parent: Record<string, unknown> | unknown[];
    key: string | number;
    target: string;
  }[] = [];

  function visit(value: unknown, scope: string): unknown {
    if (!value || typeof value !== "object" || value instanceof Uint8Array)
      return value;
    if (Array.isArray(value))
      return value.map((member) => visit(member, scope));
    const source = value as Record<string, unknown>;
    const schema = protocolMessageSchema(value);
    if (schema) {
      // Every WKT is converted as one declared JSON boundary; Struct contents stay opaque.
      if (schema.typeName.startsWith("google.protobuf."))
        return toJson(schema, value as Message);
      const output = toJson(schema, value as Message) as Record<
        string,
        unknown
      >;
      const subjectField = schema.fields.find(
        (field) => protocolReferenceKind(field) === "subject",
      );
      if (
        subjectField &&
        typeof source[subjectField.localName] === "string" &&
        source[subjectField.localName]
      )
        scope = source[subjectField.localName] as string;

      for (const field of schema.fields) {
        const selected = field.oneof
          ? (source[field.oneof.localName] as
              { case?: string; value?: unknown } | undefined)
          : undefined;
        if (field.oneof && selected?.case !== field.localName) continue;
        const member = field.oneof ? selected?.value : source[field.localName];
        const key = field.jsonName;
        if (
          field.fieldKind === "message" &&
          member &&
          typeof member === "object"
        )
          output[key] = visit(member, scope);
        else if (
          field.fieldKind === "list" &&
          field.listKind === "message" &&
          Array.isArray(member)
        )
          output[key] = member.map((child) => visit(child, scope));
        else if (
          field.fieldKind === "map" &&
          field.mapKind === "message" &&
          member &&
          typeof member === "object"
        )
          output[key] = Object.fromEntries(
            Object.entries(member).map(([name, child]) => [
              name,
              visit(child, scope),
            ]),
          );

        const declaredKind = protocolReferenceKind(field);
        if (!declaredKind) continue;
        const kind = declaredKind === "$kind" ? source.kind : declaredKind;
        if (typeof kind !== "string" || !kind) continue;
        let display: unknown;
        let hasDisplay = false;
        const address = (canonical: unknown) => {
          if (typeof canonical !== "string" || !canonical) return undefined;
          const target = {
            subjectId: kind === "subject" ? canonical : scope,
            canonical: { kind, value: canonical },
          };
          if (!target.subjectId) return undefined;
          const id = JSON.stringify(target);
          targets.set(id, target);
          return id;
        };
        if (Array.isArray(member)) {
          const displayValues: unknown[] = member.map((canonical, index) => {
            const id = address(canonical);
            if (id) {
              hasDisplay = true;
              slots.push({ parent: displayValues, key: index, target: id });
            }
            return undefined;
          });
          if (hasDisplay) display = displayValues;
        } else {
          const id = address(member);
          if (id) {
            hasDisplay = true;
            display = undefined;
            const references = (output.displayReferences ??= {}) as Record<
              string,
              unknown
            >;
            slots.push({
              parent: references,
              key: field.localName,
              target: id,
            });
          }
        }
        if (hasDisplay) {
          const references = (output.displayReferences ??= {}) as Record<
            string,
            unknown
          >;
          if (display !== undefined) references[field.localName] = display;
          else references[field.localName] = undefined;
          if (Array.isArray(display)) references[field.localName] = display;
        }
      }
      return output;
    }

    // CLI-owned DTOs may contain messages; they have no protocol meaning of their own.
    return Object.fromEntries(
      Object.entries(source).map(([key, member]) => [
        key,
        visit(member, scope),
      ]),
    );
  }

  const result = visit(input, env.subjectId);
  const requests = [...targets.values()];
  const addresses = new Map<string, string>();
  for (let start = 0; start < requests.length; start += 2048) {
    const response = await env.client.identity.addresses({
      targets: requests.slice(start, start + 2048),
    });
    for (const address of response.addresses)
      if (address.status === "BOUND" && address.target && address.lexicalRef)
        addresses.set(JSON.stringify(address.target), address.lexicalRef);
  }
  for (const slot of slots) {
    const address = addresses.get(slot.target);
    if (address !== undefined)
      (slot.parent as Record<string | number, unknown>)[slot.key] = address;
  }
  return result;
}

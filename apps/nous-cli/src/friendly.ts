// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { protocolSchema, protocolReferenceKind } from "@nous-wave/client/data";
import type { CliEnvironment } from "./runtime.js";

/** Only protocol-declared references are addressed; user JSON and scalar text remain opaque. */
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
    const schema = protocolSchema(value);
    const subjectField = schema?.fields.find(
      (field) => protocolReferenceKind(field) === "subject",
    );
    if (
      subjectField &&
      typeof source[subjectField.localName] === "string" &&
      source[subjectField.localName]
    )
      scope = source[subjectField.localName] as string;
    const output = Object.fromEntries(
      Object.entries(source).map(([key, member]) => [
        key,
        visit(member, scope),
      ]),
    );
    for (const field of schema?.fields ?? []) {
      const declaredKind = protocolReferenceKind(field);
      if (!declaredKind) continue;
      const kind = declaredKind === "$kind" ? source.kind : declaredKind;
      if (typeof kind !== "string" || !kind) continue;
      let parent: Record<string, unknown> | unknown[] = output;
      let key: string | number = field.localName;
      if (field.oneof) {
        const oneof = output[field.oneof.localName] as
          { case?: string; value?: unknown } | undefined;
        if (oneof?.case !== field.localName) continue;
        parent = oneof;
        key = "value";
      }
      const member = parent[key as keyof typeof parent];
      const address = (
        container: Record<string, unknown> | unknown[],
        addressKey: string | number,
        canonical: unknown,
      ) => {
        if (typeof canonical !== "string" || !canonical) return;
        const target = {
          subjectId: kind === "subject" ? canonical : scope,
          canonical: { kind, value: canonical },
        };
        if (!target.subjectId) return;
        const id = JSON.stringify(target);
        targets.set(id, target);
        slots.push({ parent: container, key: addressKey, target: id });
      };
      if (Array.isArray(member))
        member.forEach((canonical, index) => address(member, index, canonical));
      else address(parent, key, member);
    }
    return output;
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

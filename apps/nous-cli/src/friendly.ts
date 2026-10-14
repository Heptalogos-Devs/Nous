// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { referenceSlots, messageData } from "@nous-wave/protocol/messages";
import type { CliEnvironment } from "./runtime.js";
type Target = { subjectId: string; canonical: { kind: string; value: string } };
const targetKey = ({ subjectId, canonical }: Target) =>
  JSON.stringify([subjectId, canonical.kind, canonical.value]);

/** Presentation consumes native descriptors and existing query addresses; JSON leaves stay opaque. */
export async function friendlyOutput(
  input: unknown,
  env: CliEnvironment,
): Promise<unknown> {
  const result = structuredClone(input);
  const slots = referenceSlots(result);
  if (slots.some((slot) => !slot.subjectId && slot.kind !== "subject")) {
    const subjectId = await env.subjectId;
    for (const slot of slots) if (!slot.subjectId) slot.subjectId = subjectId;
  }
  const addresses = new Map<string, string>();
  const query = input as
    | {
        results?: {
          lexicalRef?: string;
          ref?: { kind: string; value: string };
        }[];
      }
    | undefined;
  for (const hit of query?.results ?? [])
    if (hit.ref && hit.lexicalRef)
      addresses.set(
        targetKey({ subjectId: await env.subjectId, canonical: hit.ref }),
        hit.lexicalRef,
      );
  const targets = new Map<string, Target>();
  for (const { subjectId, kind, value } of slots) {
    const target = { subjectId, canonical: { kind, value } };
    const key = targetKey(target);
    if (subjectId && !addresses.has(key)) targets.set(key, target);
  }
  const requests = [...targets.values()];
  for (let start = 0; start < requests.length; start += 2048) {
    const response = await env.client.identity.getIdentityAddresses({
      targets: requests.slice(start, start + 2048),
    });
    for (const address of response.addresses)
      if (
        address.status === "BOUND" &&
        address.target?.canonical &&
        address.lexicalRef
      )
        addresses.set(
          targetKey({
            subjectId: address.target.subjectId,
            canonical: address.target.canonical,
          }),
          address.lexicalRef,
        );
  }
  for (const slot of slots) {
    const address = addresses.get(
      targetKey({ subjectId: slot.subjectId, canonical: slot }),
    );
    if (address) slot.parent[slot.key] = address;
  }
  return messageData(result);
}

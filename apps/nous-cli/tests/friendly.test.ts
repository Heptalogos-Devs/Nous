// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { create, fromJson } from "@bufbuild/protobuf";
import { StructSchema } from "@bufbuild/protobuf/wkt";
import {
  MemorySchema,
  CognitiveRefSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { protocolReferences } from "@nous-wave/protocol/messages";
import { GetIdentityAddressesResponseSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/identity_pb.js";
import { expect, it, vi } from "vitest";
import { friendlyOutput } from "../src/friendly.js";
import type { CliEnvironment } from "../src/runtime.js";
import { renderText } from "../src/output.js";

it("preserves arbitrary JSON, exact UUID/ref bodies, labels and diagnostics without address requests", async () => {
  const id = "33333333-3333-4333-8333-333333333333";
  const addresses = vi.fn();
  const bodies = {
    text: id,
    title: `memory:${id}`,
    synopsis: `entity:${id}`,
    message: `source_region:${id}`,
    nested: [
      { subjectId: id, kind: "source_region", value: id, contextText: id },
    ],
    diagnostics: { queryId: id, text: id },
  };
  const value = fromJson(StructSchema, bodies);
  expect(protocolReferences(value, StructSchema)).toEqual([]);
  expect(
    await friendlyOutput(value, {
      subjectId: id,
      client: { identity: { getIdentityAddresses: addresses } },
    } as unknown as CliEnvironment),
  ).toEqual(bodies);
  expect(addresses).not.toHaveBeenCalled();
});

it("reads one deduplicated directory batch for schema-declared references and oneof evidence", async () => {
  const id = "33333333-3333-4333-8333-333333333333";
  const addresses = vi.fn(
    async (request: {
      targets: {
        subjectId: string;
        canonical: { kind: string; value: string };
      }[];
    }) =>
      create(GetIdentityAddressesResponseSchema, {
        addresses: request.targets.map((target) => ({
          target,
          status: "BOUND",
          lexicalRef:
            target.canonical.kind === "occurrence"
              ? "obs:fakoh-todud-rovor"
              : target.canonical.kind === "subject"
                ? "sub:tujap-vibuz-gozod"
                : "src:vahol-gutij-nakur",
        })),
      }),
  );
  const bind = vi.fn();
  const cognition = create(MemorySchema, {
    subjectId: id,
    text: `Original ${id}`,
    title: `memory:${id}`,
    basis: [
      {
        basis: {
          case: "evidence",
          value: {
            occurrenceId: id,
            locator: { case: "sourceRegionId", value: id },
          },
        },
      },
    ],
  });
  const input = {
    cognition,
    sources: [
      {
        reference: create(CognitiveRefSchema, {
          kind: "source_region",
          value: id,
        }),
      },
    ],
  };
  expect(protocolReferences(cognition, MemorySchema)).toEqual([
    { kind: "subject", value: id },
    { kind: "occurrence", value: id },
    { kind: "source_region", value: id },
  ]);
  expect(
    protocolReferences(input.sources[0]!.reference, CognitiveRefSchema),
  ).toEqual([{ kind: "source_region", value: id }]);
  const result = await friendlyOutput(input, {
    subjectId: id,
    client: {
      identity: { getIdentityAddresses: addresses, bindIdentity: bind },
    },
  } as unknown as CliEnvironment);
  expect(result).toMatchObject({
    cognition: {
      text: `Original ${id}`,
      title: `memory:${id}`,
      basis: [
        {
          evidence: {
            occurrenceId: "obs:fakoh-todud-rovor",
            sourceRegionId: "src:vahol-gutij-nakur",
          },
        },
      ],
    },
    sources: [{ reference: { value: "src:vahol-gutij-nakur" } }],
  });
  expect(addresses).toHaveBeenCalledTimes(1);
  expect(addresses.mock.calls[0]![0].targets).toHaveLength(3);
  expect(bind).not.toHaveBeenCalled();
  expect(
    cognition.basis[0]?.basis.case === "evidence" &&
      cognition.basis[0].basis.value.occurrenceId,
  ).toBe(id);
  expect(renderText(result)).toContain("src:vahol-gutij-nakur");
});

it("reuses native query addresses without requesting them again", async () => {
  const id = "33333333-3333-4333-8333-333333333333";
  const addresses = vi.fn();
  const input = {
    results: [
      {
        lexicalRef: "memrev:fakoh-todud-rovor",
        ref: create(CognitiveRefSchema, { kind: "memory_revision", value: id }),
      },
    ],
  };
  expect(
    await friendlyOutput(input, {
      subjectId: id,
      client: { identity: { getIdentityAddresses: addresses } },
    } as unknown as CliEnvironment),
  ).toMatchObject({
    results: [
      {
        lexicalRef: "memrev:fakoh-todud-rovor",
        ref: { value: "memrev:fakoh-todud-rovor" },
      },
    ],
  });
  expect(addresses).not.toHaveBeenCalled();
  expect(input.results[0]!.ref.value).toBe(id);
});

it("shows actual opaque fields and envelope notices without guessing reference or timestamp shapes", () => {
  const value = {
    schemaVersion: "nous.cli.v1",
    data: {
      body: {
        kind: "user label",
        value: "user value",
        seconds: 123,
        $typeName: "literal source key",
      },
    },
    notices: [
      {
        code: "STATE_UNSAVED",
        message: "Continue using the returned exact reference",
      },
    ],
  };
  const text = renderText(value);
  expect(text).toContain("kind: user label");
  expect(text).toContain("seconds: 123");
  expect(text).toContain("$typeName: literal source key");
  expect(text).toContain("STATE_UNSAVED");
});

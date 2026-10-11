// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { create, fromJson } from "@bufbuild/protobuf";
import { StructSchema } from "@bufbuild/protobuf/wkt";
import {
  MemorySchema,
  CognitiveRefSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { protocolData, protocolReferences } from "@nous-wave/client/data";
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
  const value = protocolData(fromJson(StructSchema, bodies));
  expect(protocolReferences(value, StructSchema)).toEqual([]);
  expect(
    await friendlyOutput(value, {
      subjectId: id,
      client: { identity: { addresses } },
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
    }) => ({
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
  const cognition = protocolData(
    create(MemorySchema, {
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
    }),
  );
  const input = {
    cognition,
    sources: [
      {
        reference: protocolData(
          create(CognitiveRefSchema, { kind: "source_region", value: id }),
        ),
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
    client: { identity: { addresses, bind } },
  } as unknown as CliEnvironment);
  expect(result).toMatchObject({
    cognition: {
      text: `Original ${id}`,
      title: `memory:${id}`,
      basis: [
        {
          basis: {
            value: {
              occurrenceId: "obs:fakoh-todud-rovor",
              locator: { value: "src:vahol-gutij-nakur" },
            },
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

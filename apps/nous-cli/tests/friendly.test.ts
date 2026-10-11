// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { create, fromJson } from "@bufbuild/protobuf";
import { StructSchema } from "@bufbuild/protobuf/wkt";
import {
  MemorySchema,
  CognitiveRefSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import {
  GetIdentityAddressesResponseSchema,
  IdentityAddressSchema,
  IdentityAddressTargetSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/identity_pb.js";
import { protocolReferences } from "@nous-wave/client/data";
import { expect, it, vi } from "vitest";
import { friendlyOutput } from "../src/friendly.js";
import type { CliEnvironment } from "../src/runtime.js";
import { renderText } from "../src/output.js";

it("preserves arbitrary JSON with protocol-looking keys without address requests", async () => {
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
      client: { identity: { addresses } },
    } as unknown as CliEnvironment),
  ).toEqual(bodies);
  expect(addresses).not.toHaveBeenCalled();
});

it("keeps canonical reference data and adds lexical refs only as display metadata", async () => {
  const id = "33333333-3333-4333-8333-333333333333";
  const addresses = vi.fn(
    async (request: {
      targets: {
        subjectId: string;
        canonical: { kind: string; value: string };
      }[];
    }) =>
      create(GetIdentityAddressesResponseSchema, {
        addresses: request.targets.map((target) =>
          create(IdentityAddressSchema, {
            target: create(IdentityAddressTargetSchema, {
              subjectId: target.subjectId,
              canonical: create(CognitiveRefSchema, target.canonical),
            }),
            status: "BOUND",
            lexicalRef:
              target.canonical.kind === "occurrence"
                ? "obs:fakoh-todud-rovor"
                : target.canonical.kind === "subject"
                  ? "sub:tujap-vibuz-gozod"
                  : "src:vahol-gutij-nakur",
          }),
        ),
      }),
  );
  const bind = vi.fn();
  const cognition = create(MemorySchema, {
    header: { subjectId: id },
    details: { cognitiveRole: "declarative", accessibilityMode: "auto" },
    revision: {
      details: {
        semanticRole: "test",
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
      },
    },
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
    client: { identity: { addresses, bind } },
  } as unknown as CliEnvironment);
  expect(result).toMatchObject({
    cognition: {
      header: {
        subjectId: id,
        displayReferences: { subjectId: "sub:tujap-vibuz-gozod" },
      },
      revision: {
        details: {
          text: `Original ${id}`,
          title: `memory:${id}`,
          basis: [
            {
              evidence: {
                occurrenceId: id,
                sourceRegionId: id,
                displayReferences: {
                  occurrenceId: "obs:fakoh-todud-rovor",
                  sourceRegionId: "src:vahol-gutij-nakur",
                },
              },
            },
          ],
        },
      },
    },
    sources: [
      {
        reference: {
          value: id,
          displayReferences: { value: "src:vahol-gutij-nakur" },
        },
      },
    ],
  });
  expect(addresses).toHaveBeenCalledTimes(1);
  expect(addresses.mock.calls[0]![0].targets).toHaveLength(3);
  expect(bind).not.toHaveBeenCalled();
  expect(
    cognition.revision?.details?.basis[0]?.basis.case === "evidence" &&
      cognition.revision.details.basis[0].basis.value.occurrenceId,
  ).toBe(id);
  expect(renderText(result)).toContain("src:vahol-gutij-nakur");
  expect(renderText(result)).toContain("subjectId: sub:tujap-vibuz-gozod");
});

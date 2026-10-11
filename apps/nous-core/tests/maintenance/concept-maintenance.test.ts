// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { expect, it, vi } from "vitest";
import { create } from "@bufbuild/protobuf";
import { MaintenancePlanSchema } from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/longitudinal_pb.js";
import {
  TagSchema,
  AssociationSchema,
  SplitTagResponseSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/management_pb.js";
import { ProducerSignatureSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import type { KernelClient } from "../../src/kernel-client.js";
import { conceptMaintenanceSchema } from "../../src/model/schemas/concept-maintenance.js";
import {
  executeConceptMaintenance,
  type ConceptActionResult,
} from "../../src/maintenance/concept-maintenance.js";
const content = {
  label: "Release approval",
  description: "Reviewer signoff before rollout",
  kind_hint: "procedure",
};
const plan = create(MaintenancePlanSchema, {
  subjectId: "subject",
  conceptCatalog: {
    maxSuggestions: 4,
    configDigest: "frozen",
    references: [
      {
        key: "c0",
        reference: { kind: "memory_revision", value: "memory-revision" },
      },
      { key: "t0", reference: { kind: "tag", value: "tag-old" } },
    ],
    tags: [
      {
        key: "t0",
        target: { tagId: "tag-old", expectedRevisionId: "tag-revision" },
      },
    ],
    basis: [
      {
        key: "s0",
        basis: {
          basis: {
            case: "revision",
            value: {
              basis: {
                case: "cognitionDependency",
                value: {
                  targetRevision: {
                    kind: "memory_revision",
                    value: "memory-revision",
                  },
                  basisRole: "direct",
                },
              },
            },
          },
        },
      },
    ],
  },
});
const producer = create(ProducerSignatureSchema, {
  operation: "concept_maintenance_text",
});
const workflow = "40000000-0000-4000-8000-000000000001";
const createTag = {
  action: "create_tag",
  key: "new_0",
  cognitionKeys: ["c0"],
  content,
  basisKeys: ["s0"],
  reason: "An accepted recurring procedure",
};
const attach = {
  action: "attach_tag",
  cognitionKey: "c0",
  tagKey: "new_0",
  basisKeys: ["s0"],
  reason: "This cognition expresses the concept",
};
function fixture() {
  const formTag = vi.fn(
    async (_request: Parameters<KernelClient["concepts"]["createTag"]>[0]) =>
      create(TagSchema, { tagId: "tag-new", currentRevisionId: "tag-new-r1" }),
  );
  const association = vi.fn(
    async (
      _request: Parameters<KernelClient["concepts"]["createAssociation"]>[0],
    ) => create(AssociationSchema, { associationId: "association-new" }),
  );
  const revise = vi.fn(
    async (_request: Parameters<KernelClient["concepts"]["reviseTag"]>[0]) =>
      create(TagSchema, { tagId: "tag-old", currentRevisionId: "tag-old-r2" }),
  );
  const split = vi.fn(async () =>
    create(SplitTagResponseSchema, {
      children: [
        { tagId: "child-a", currentRevisionId: "child-a-r1" },
        { tagId: "child-b", currentRevisionId: "child-b-r1" },
      ],
    }),
  );
  const kernel = {
    concepts: {
      createTag: formTag,
      createAssociation: association,
      reviseTag: revise,
      splitTag: split,
    },
  } as unknown as KernelClient;
  let progress: ConceptActionResult[] = [];
  const save = vi.fn(async (results: ConceptActionResult[]) => {
    progress = [...results];
  });
  const run = (actions: unknown[]) =>
    executeConceptMaintenance(
      kernel,
      plan,
      conceptMaintenanceSchema.parse({ actions }),
      workflow,
      producer,
      {},
      progress,
      save,

      (_id, commit) => commit(),
    );
  return {
    kernel,
    createTag: formTag,
    association,
    revise,
    split,
    save,
    run,
    progress: () => progress,
  };
}
it("creates a concept and attaches its returned identity through typed owner APIs", async () => {
  const f = fixture();
  const outcome = await f.run([createTag, attach]);
  expect(outcome.status).toBe("committed");
  expect(f.createTag.mock.calls[0]?.[0].producer).toEqual(producer);
  expect(f.association.mock.calls[0]?.[0].association?.from?.value).toBe(
    "memory-revision",
  );
  expect(f.association.mock.calls[0]?.[0].association?.to?.value).toBe(
    "tag-new",
  );
  expect(f.association.mock.calls[0]?.[0].association?.relationKind).toBe(
    "tag_attachment",
  );
  expect(f.createTag.mock.calls[0]?.[0].operationId).not.toBe(
    f.association.mock.calls[0]?.[0].operationId,
  );
  expect(f.progress()).toHaveLength(2);
});

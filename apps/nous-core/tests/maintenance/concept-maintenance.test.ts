// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import {
  domainError,
  DomainErrorCode,
  ErrorRecovery,
} from "@nous-wave/client/errors";
import { create } from "@bufbuild/protobuf";
import { Code, ConnectError } from "@connectrpc/connect";
import { expect, it, vi } from "vitest";
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
it("keeps independent commits and skips only dependencies of an invalid creation", async () => {
  const f = fixture();
  f.createTag.mockRejectedValueOnce(
    new ConnectError("Invalid concept", Code.InvalidArgument),
  );
  const result = await f.run([
    createTag,
    attach,
    {
      action: "revise_tag",
      tagKey: "t0",
      content,
      basisKeys: ["s0"],
      reason: "Clarify scope",
    },
    {
      action: "create_association",
      fromKey: "c0",
      toKey: "t0",
      relation: "assoc.related",
      basisKeys: ["invented"],
      reason: "Invented support",
    },
  ]);
  expect(result.status).toBe("partial");
  expect(result.actions.map((a) => a.status)).toEqual([
    "rejected_invalid",
    "skipped_dependency",
    "committed",
    "rejected_invalid",
  ]);
  expect(f.association).not.toHaveBeenCalled();
  expect(f.revise).toHaveBeenCalledTimes(1);
});
it("resumes saved results with stable IDs after a transport failure without recreating the Tag", async () => {
  const f = fixture();
  f.association.mockRejectedValueOnce(
    new ConnectError("Response lost", Code.Unavailable),
  );
  await expect(f.run([createTag, attach])).rejects.toThrow("Response lost");
  expect(f.progress()).toHaveLength(1);
  const result = await f.run([createTag, attach]);
  expect(result.status).toBe("committed");
  expect(f.createTag).toHaveBeenCalledTimes(1);
  expect(f.association).toHaveBeenCalledTimes(2);
  expect(f.association.mock.calls[0]?.[0].operationId).toBe(
    f.association.mock.calls[1]?.[0].operationId,
  );
  expect(f.association.mock.calls[1]?.[0].association?.to?.value).toBe(
    "tag-new",
  );
});
it("maps split children and later revisions to actual returned identities", async () => {
  const f = fixture();
  const result = await f.run([
    {
      action: "split_tag",
      tagKey: "t0",
      children: [
        { key: "new_a", content },
        { key: "new_b", content: { ...content, label: "Rollout procedure" } },
      ],
      basisKeys: ["s0"],
      reason: "Distinct concepts",
    },
    { ...attach, tagKey: "new_b" },
    {
      action: "revise_tag",
      tagKey: "new_a",
      content,
      basisKeys: ["s0"],
      reason: "Clarify child",
    },
  ]);
  expect(result.status).toBe("committed");
  expect(f.association.mock.calls[0]?.[0].association?.to?.value).toBe(
    "child-b",
  );
  expect(f.revise.mock.calls[0]?.[0].target?.tagId).toBe("child-a");
  expect(f.revise.mock.calls[0]?.[0].target?.expectedRevisionId).toBe(
    "child-a-r1",
  );
});
it("continues after a stale item and exposes owner invariant failure", async () => {
  const f = fixture();
  f.association.mockRejectedValueOnce(
    domainError("Stale endpoint", Code.Aborted, {
      code: DomainErrorCode.STALE_REVISION,
      recovery: ErrorRecovery.REFRESH_STATE,
    }),
  );
  const result = await f.run([{ ...attach, tagKey: "t0" }, createTag]);
  expect(result.status).toBe("partial");
  expect(result.actions[0]?.status).toBe("stale");
  const fatal = fixture();
  fatal.createTag.mockRejectedValueOnce(
    new ConnectError("Invariant", Code.Internal),
  );
  await expect(fatal.run([createTag, attach])).rejects.toThrow("Invariant");
  expect(fatal.association).not.toHaveBeenCalled();
  expect(fatal.progress()).toHaveLength(0);
});

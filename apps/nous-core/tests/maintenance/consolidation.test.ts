// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import {
  domainError,
  DomainErrorCode,
  ErrorRecovery,
} from "@nous-wave/client/errors";
import { expect, it, vi } from "vitest";
import { create } from "@bufbuild/protobuf";
import { Code, ConnectError } from "@connectrpc/connect";
import { MaintenancePlanSchema } from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/longitudinal_pb.js";
import { ProducerSignatureSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import type { KernelClient } from "../../src/kernel-client.js";
import { consolidationSchema } from "../../src/model/schemas/consolidation.js";
import {
  executeConsolidation,
  type ConsolidationActionResult,
} from "../../src/maintenance/consolidation.js";
const occurrence = "10000000-0000-4000-8000-000000000001";
const revision = "20000000-0000-4000-8000-000000000001";
const operation = "40000000-0000-4000-8000-000000000001";
const plan = create(MaintenancePlanSchema, {
  subjectId: "30000000-0000-4000-8000-000000000001",
  authoritySeq: 12n,
  maxConsolidationActions: 8,
  consolidationSource: {
    reference: { kind: "episode_revision", value: revision },
    expectedEpoch: 2n,
  },
  members: [{ key: "member", occurrenceId: occurrence }],
  basis: [
    {
      key: "source",
      basis: {
        basis: {
          case: "evidence",
          value: {
            occurrenceId: occurrence,
            locator: { case: "wholeOccurrence", value: true },
            basisRole: "direct",
          },
        },
      },
    },
  ],
  candidates: [
    {
      key: "target",
      cognitiveRole: "declarative",
      formationMode: "grounded",
      eligibleBasisKeys: ["source"],
      target: {
        reference: { kind: "memory_revision", value: revision },
        objectId: "memory-owner-id",
        expectedEpoch: 7n,
      },
    },
  ],
});
const content = {
  cognitiveRole: "declarative",
  formationMode: "grounded",
  groundingMemberKey: "member",
  semanticRole: "statement",
  text: "A useful supported fact.",
  title: null,
  basisKeys: ["source"],
  entityKeys: [],
  validTime: { kind: "unknown" },
  epistemicClass: "derived",
};
const schemaContent = {
  title: null,
  structuralClaim: "A bounded pattern",
  applicability: "These observations",
  boundaryDefinition: "Only this scope",
  formationKind: "explicit_import",
  entityKeys: [],
  validTime: { kind: "unknown" },
  evidence: [{ role: "support", basisKey: "source" }],
};
function fixture() {
  const form = vi.fn(
    async (_request: Parameters<KernelClient["memory"]["formMemory"]>[0]) => ({
      revisionId: "formed-revision",
    }),
  );
  const revise = vi.fn(async () => ({ revisionId: "revised-revision" }));
  const link = vi.fn(async () => ({}));
  const schema = vi.fn(async () => ({ currentRevisionId: "schema-revision" }));
  const kernel = {
    memory: { formMemory: form, reviseMemory: revise, linkRevisions: link },
    concepts: { createCognitiveSchema: schema },
  } as unknown as KernelClient;
  let progress: ConsolidationActionResult[] = [];
  const save = vi.fn(async (results: ConsolidationActionResult[]) => {
    progress = [...results];
  });
  return { kernel, form, revise, link, schema, save, progress: () => progress };
}
it("uses canonical owner APIs and returned earlier-action refs for strong Memory relations", async () => {
  const f = fixture();
  const proposal = consolidationSchema.parse({
    actions: [
      { action: "create_memory", content },
      {
        action: "revise_memory",
        targetKey: "target",
        intent: "correct",
        content,
      },
      {
        action: "link_relation",
        from: { kind: "action", index: 0 },
        to: { kind: "action", index: 1 },
        relation: "elaborates",
      },
      { action: "create_schema", content: schemaContent },
    ],
  });
  const outcome = await executeConsolidation(
    f.kernel,
    plan,
    proposal,
    operation,
    create(ProducerSignatureSchema),
    {},
    [],
    f.save,

    (_id, commit) => commit(),
  );
  expect(outcome.status).toBe("committed");
  expect(f.form.mock.calls[0]?.[0].input?.groundingOccurrenceId).toBe(
    occurrence,
  );
  expect(f.revise).toHaveBeenCalledWith(
    expect.objectContaining({
      memoryId: "memory-owner-id",
      expectedObjectEpoch: 7n,
    }),
    {},
  );
  expect(f.link).toHaveBeenCalledWith(
    expect.objectContaining({
      fromRevisionId: "formed-revision",
      toRevisionId: "revised-revision",
    }),
    {},
  );
  expect(new Set(f.progress().map((r) => r.index)).size).toBe(4);
});
it("preserves earlier commits, skips failed dependencies and continues independent suggestions", async () => {
  const f = fixture();
  const proposal = consolidationSchema.parse({
    actions: [
      { action: "create_memory", content },
      {
        action: "create_memory",
        content: { ...content, basisKeys: ["invented"] },
      },
      {
        action: "link_relation",
        from: { kind: "action", index: 1 },
        to: { kind: "action", index: 0 },
        relation: "elaborates",
      },
      { action: "create_schema", content: schemaContent },
    ],
  });
  const outcome = await executeConsolidation(
    f.kernel,
    plan,
    proposal,
    operation,
    create(ProducerSignatureSchema),
    {},
    [],
    f.save,

    (_id, commit) => commit(),
  );
  expect(outcome.status).toBe("partial");
  expect(outcome.actions.map((r) => r.status)).toEqual([
    "committed",
    "rejected_invalid",
    "skipped_dependency",
    "committed",
  ]);
  expect(f.form).toHaveBeenCalledTimes(1);
  expect(f.link).not.toHaveBeenCalled();
  expect(f.schema).toHaveBeenCalledTimes(1);
});
it("resumes saved progress on transport failure with stable operation identities", async () => {
  const f = fixture();
  f.schema.mockRejectedValueOnce(
    new ConnectError("lost response", Code.Unavailable),
  );
  const proposal = consolidationSchema.parse({
    actions: [
      { action: "create_memory", content },
      { action: "create_schema", content: schemaContent },
    ],
  });
  const run = () =>
    executeConsolidation(
      f.kernel,
      plan,
      proposal,
      operation,
      create(ProducerSignatureSchema),
      {},
      f.progress(),
      f.save,

      (_id, commit) => commit(),
    );
  await expect(run()).rejects.toThrow("lost response");
  expect(f.progress()).toHaveLength(1);
  expect((await run()).status).toBe("committed");
  expect(f.form).toHaveBeenCalledTimes(1);
  expect(f.schema.mock.calls[1]).toEqual(f.schema.mock.calls[0]);
});
it("makes a stale target terminal while independent creation still commits", async () => {
  const f = fixture();
  f.revise.mockRejectedValueOnce(
    domainError("stale epoch", Code.Aborted, {
      code: DomainErrorCode.STALE_REVISION,
      recovery: ErrorRecovery.REFRESH_STATE,
    }),
  );
  const proposal = consolidationSchema.parse({
    actions: [
      {
        action: "revise_memory",
        targetKey: "target",
        intent: "correct",
        content,
      },
      { action: "create_memory", content },
      {
        action: "link_relation",
        from: { kind: "action", index: 0 },
        to: { kind: "action", index: 1 },
        relation: "elaborates",
      },
    ],
  });
  const outcome = await executeConsolidation(
    f.kernel,
    plan,
    proposal,
    operation,
    create(ProducerSignatureSchema),
    {},
    [],
    f.save,

    (_id, commit) => commit(),
  );
  expect(outcome.actions.map((r) => r.status)).toEqual([
    "stale",
    "committed",
    "skipped_dependency",
  ]);
  expect(outcome.status).toBe("partial");
});

// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import type { MutationExecutor } from "../durable-operation.js";
import { create } from "@bufbuild/protobuf";
import { timestampFromDate } from "@bufbuild/protobuf/wkt";
import { Code, ConnectError, type CallOptions } from "@connectrpc/connect";
import { authorityRejection } from "./rejection.js";
import { z } from "zod";
import type { MaintenancePlan } from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/longitudinal_pb.js";
import { CognitiveSchemaContentSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/management_pb.js";
import {
  MemoryContentSchema,
  TemporalExtentSchema,
  type ProducerSignature,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import type { KernelClient } from "../kernel-client.js";
import type { ConsolidationProposal } from "../model/schemas/consolidation.js";
import { maintenanceActionOperationId } from "./identity.js";

type Action = ConsolidationProposal["actions"][number];
export const consolidationResultSchema = z.strictObject({
  index: z.number().int().min(0),
  status: z.enum([
    "committed",
    "no_change",
    "rejected_invalid",
    "skipped_dependency",
    "stale",
  ]),
  resultRef: z.strictObject({ kind: z.string(), value: z.string() }).nullable(),
});
export type ConsolidationActionResult = z.infer<
  typeof consolidationResultSchema
>;
function invalid(message: string): never {
  throw new ConnectError(message, Code.InvalidArgument);
}
function time(
  value: Extract<Action, { action: "create_memory" }>["content"]["validTime"],
) {
  if (value.kind === "unknown") return create(TemporalExtentSchema);
  if (value.kind === "instant")
    return create(TemporalExtentSchema, {
      value: { case: "instant", value: timestampFromDate(new Date(value.at)) },
    });
  return create(TemporalExtentSchema, {
    value: {
      case: "interval",
      value: {
        start: value.start
          ? timestampFromDate(new Date(value.start))
          : undefined,
        end: value.end ? timestampFromDate(new Date(value.end)) : undefined,
      },
    },
  });
}

/** The saved model result is executed with each owner's own transaction and receipt. */
export async function executeConsolidation(
  kernel: KernelClient,
  plan: MaintenancePlan,
  proposal: ConsolidationProposal,
  operationId: string,
  producer: ProducerSignature,
  options: CallOptions,
  progress: ConsolidationActionResult[],
  saveProgress: (results: ConsolidationActionResult[]) => Promise<void>,
  mutate: MutationExecutor,
) {
  if (
    !plan.consolidationSource ||
    proposal.actions.length > plan.maxConsolidationActions
  )
    invalid("Consolidation source or action envelope is invalid");
  const basis = new Map(plan.basis.map((entry) => [entry.key, entry.basis]));
  const entities = new Map(
    plan.entities.map((entry) => [entry.key, entry.entityRef]),
  );
  const candidates = new Map(
    plan.candidates.map((entry) => [entry.key, entry]),
  );
  const members = new Map(
    plan.members.map((entry) => [entry.key, entry.occurrenceId]),
  );
  const results = [...progress];
  const selectedBasis = (keys: string[], eligible?: string[]) => {
    if (
      new Set(keys).size !== keys.length ||
      keys.some((key) => eligible && !eligible.includes(key))
    )
      invalid("Duplicate or self-dependent consolidation basis");
    return keys.map(
      (key) =>
        basis.get(key) ??
        invalid("Consolidation support key is outside the catalog"),
    );
  };
  const selectedEntities = (keys: string[]) => {
    if (new Set(keys).size !== keys.length)
      invalid("Duplicate consolidation entities");
    return keys.map(
      (key) =>
        entities.get(key) ??
        invalid("Consolidation entity key is outside the catalog"),
    );
  };
  const target = (key: string, kind: string) => {
    const candidate =
      candidates.get(key) ??
      invalid("Consolidation target is outside current context");
    if (
      !candidate.target?.objectId ||
      candidate.target.reference?.kind !== kind
    )
      invalid("Consolidation target has the wrong domain or no owner identity");
    return candidate;
  };
  const memory = (
    content: Extract<Action, { action: "create_memory" }>["content"],
    eligible?: string[],
  ) =>
    create(MemoryContentSchema, {
      producer,
      cognitiveRole: content.cognitiveRole,
      formationMode: content.formationMode,
      groundingOccurrenceId:
        content.groundingMemberKey === null
          ? undefined
          : (members.get(content.groundingMemberKey) ??
            invalid("Grounding key is outside the member catalog")),
      semanticRole: content.semanticRole,
      text: content.text,
      title: content.title ?? undefined,
      basis: selectedBasis(content.basisKeys, eligible),
      aboutness: selectedEntities(content.entityKeys),
      tags: [],
      validTime: time(content.validTime),
      epistemicClass: content.epistemicClass,
    });
  const schema = (
    content: Extract<Action, { action: "create_schema" }>["content"],
    eligible?: string[],
  ) =>
    create(CognitiveSchemaContentSchema, {
      producer,
      title: content.title ?? "",
      structuralClaim: content.structuralClaim,
      applicabilityScope: {
        description: content.applicability,
        aboutness: selectedEntities(content.entityKeys),
        tags: [],
        validTime: time(content.validTime),
      },
      boundaryDefinition: content.boundaryDefinition,
      formationKind: content.formationKind,
      evidenceLinks: content.evidence.map((link) => ({
        role: link.role,
        basis: selectedBasis([link.basisKey], eligible)[0],
      })),
    });
  const endpoint = (
    value: Extract<Action, { action: "link_relation" }>["from"],
    index: number,
  ) => {
    if (value.kind === "candidate")
      return target(value.key, "memory_revision").target!.reference!;
    if (value.index >= index)
      invalid("Consolidation relation requires an earlier action");
    const prior = proposal.actions[value.index];
    if (prior?.action !== "create_memory" && prior?.action !== "revise_memory")
      invalid("Memory relations require Memory action endpoints");
    return results[value.index]?.resultRef ?? null;
  };
  for (let index = results.length; index < proposal.actions.length; index++) {
    const action = proposal.actions[index]!;
    const id = maintenanceActionOperationId(operationId, index, action.action);
    let result: ConsolidationActionResult = {
      index,
      status: "no_change",
      resultRef: null,
    };
    try {
      switch (action.action) {
        case "skip":
          break;
        case "create_memory": {
          const value = await mutate(id, () =>
            kernel.memory.formMemory(
              {
                operationId: id,
                subjectId: plan.subjectId,
                input: memory(action.content),
              },
              options,
            ),
          );
          result = {
            index,
            status: "committed",
            resultRef: { kind: "memory_revision", value: value.revisionId },
          };
          break;
        }
        case "revise_memory": {
          const candidate = target(action.targetKey, "memory_revision");
          if (candidate.cognitiveRole !== action.content.cognitiveRole)
            invalid("Memory revision cannot change cognitive role");
          const value = await mutate(id, () =>
            kernel.memory.reviseMemory(
              {
                operationId: id,
                subjectId: plan.subjectId,
                memoryId: candidate.target!.objectId,
                expectedObjectEpoch: candidate.target!.expectedEpoch,
                intent: action.intent,
                input: memory(action.content, candidate.eligibleBasisKeys),
              },
              options,
            ),
          );
          result = {
            index,
            status: "committed",
            resultRef: { kind: "memory_revision", value: value.revisionId },
          };
          break;
        }
        case "create_schema": {
          const content = schema(action.content);
          const value = await mutate(id, () =>
            kernel.concepts.createCognitiveSchema(
              {
                operationId: id,
                subjectId: plan.subjectId,
                schema: content,
              },
              options,
            ),
          );
          result = {
            index,
            status: "committed",
            resultRef: {
              kind: "cognitive_schema_revision",
              value: value.currentRevisionId,
            },
          };
          break;
        }
        case "revise_schema": {
          const candidate = target(
            action.targetKey,
            "cognitive_schema_revision",
          );
          if (candidate.formationMode !== action.content.formationKind)
            invalid("Schema revision cannot change formation kind");
          const value = await mutate(id, () =>
            kernel.concepts.reviseCognitiveSchema(
              {
                operationId: id,
                subjectId: plan.subjectId,
                schemaId: candidate.target!.objectId,
                expectedObjectEpoch: candidate.target!.expectedEpoch,
                intent: action.intent,
                schema: schema(action.content, candidate.eligibleBasisKeys),
                copyLinkIds: [],
              },
              options,
            ),
          );
          result = {
            index,
            status: "committed",
            resultRef: {
              kind: "cognitive_schema_revision",
              value: value.currentRevisionId,
            },
          };
          break;
        }
        case "link_relation": {
          const from = endpoint(action.from, index),
            to = endpoint(action.to, index);
          if (!from || !to) {
            result.status = "skipped_dependency";
            break;
          }
          await mutate(id, () =>
            kernel.memory.linkRevisions(
              {
                operationId: id,
                subjectId: plan.subjectId,
                fromRevisionId: from.value,
                toRevisionId: to.value,
                relation: action.relation,
              },
              options,
            ),
          );
          result = {
            index,
            status: "committed",
            resultRef: { kind: from.kind, value: from.value },
          };
          break;
        }
      }
    } catch (error) {
      const rejection = authorityRejection(error);
      if (!rejection) throw error;
      result.status = rejection;
    }
    results.push(result);
    await saveProgress(results);
  }
  const committed = results.some((result) => result.status === "committed");
  const failed = results.some((result) =>
    ["rejected_invalid", "skipped_dependency", "stale"].includes(result.status),
  );
  const status = committed
    ? failed
      ? "partial"
      : "committed"
    : failed
      ? results.some((r) => r.status === "stale")
        ? "stale"
        : "rejected_invalid"
      : "no_change";
  return { status, actions: results };
}

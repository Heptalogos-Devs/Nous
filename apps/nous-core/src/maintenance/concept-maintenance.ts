// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { z } from "zod";
import { Code, ConnectError, type CallOptions } from "@connectrpc/connect";
import { authorityRejection } from "./rejection.js";
import type { MaintenancePlan } from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/longitudinal_pb.js";
import type { ProducerSignature } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import type { KernelClient } from "../kernel-client.js";
import { conceptMaintenanceSchema } from "../model/schemas/concept-maintenance.js";
import { maintenanceActionOperationId } from "./identity.js";
import type { MutationExecutor } from "../durable-operation.js";

export const conceptActionResultSchema = z.strictObject({
  index: z.number().int().nonnegative(),
  status: z.enum([
    "committed",
    "no_change",
    "rejected_invalid",
    "skipped_dependency",
    "stale",
  ]),
  resultRef: z.strictObject({ kind: z.string(), value: z.string() }).nullable(),
  tagResults: z.array(
    z.strictObject({
      key: z.string(),
      tagId: z.string(),
      revisionId: z.string(),
    }),
  ),
});
export type ConceptActionResult = z.infer<typeof conceptActionResultSchema>;
class MissingConceptDependency extends Error {}
const invalid = (message: string): never => {
  throw new ConnectError(message, Code.InvalidArgument);
};
export async function executeConceptMaintenance(
  kernel: KernelClient,
  plan: MaintenancePlan,
  proposal: z.infer<typeof conceptMaintenanceSchema>,
  operation: string,
  producer: ProducerSignature,
  options: CallOptions,
  progress: ConceptActionResult[],
  saveProgress: (results: ConceptActionResult[]) => Promise<void>,
  mutate: MutationExecutor,
) {
  const catalog = plan.conceptCatalog ?? invalid("Missing concept catalog");
  if (proposal.actions.length > catalog.maxSuggestions)
    invalid("Concept suggestions exceed the frozen policy");
  if (
    proposal.actions.some((a) => a.action === "no_change") &&
    proposal.actions.length !== 1
  )
    invalid("No-change must stand alone");
  const references = new Map(
    catalog.references
      .filter((r) => r.reference)
      .map((r) => [r.key, r.reference!]),
  );
  const tags = new Map(
    catalog.tags.filter((t) => t.target).map((t) => [t.key, { ...t.target! }]),
  );
  const associations = new Map(catalog.associations.map((a) => [a.key, a]));
  const basis = new Map(
    catalog.basis.filter((s) => s.basis).map((s) => [s.key, s.basis!]),
  );
  const results = [...progress];
  const applyTags = (result: ConceptActionResult) => {
    for (const tag of result.tagResults) {
      tags.set(tag.key, {
        $typeName: "nous.wave.v1alpha1.TagRevisionTarget",
        tagId: tag.tagId,
        expectedRevisionId: tag.revisionId,
      });
      references.set(tag.key, {
        $typeName: "nous.wave.v1alpha1.CognitiveRef",
        kind: "tag",
        value: tag.tagId,
      });
    }
  };
  for (const result of results) applyTags(result);
  const tag = (key: string, index: number) => {
    const target = tags.get(key);
    if (target) return target;
    const earlier = proposal.actions
      .slice(0, index)
      .some((a) =>
        a.action === "create_tag"
          ? a.key === key
          : a.action === "split_tag"
            ? a.children.some((c) => c.key === key)
            : false,
      );
    if (earlier) throw new MissingConceptDependency(key);
    return invalid("Unknown concept Tag key");
  };
  const endpoint = (key: string, index: number) => {
    if (key.startsWith("new_")) {
      const target = tag(key, index);
      return {
        $typeName: "nous.wave.v1alpha1.CognitiveRef" as const,
        kind: "tag",
        value: target.tagId,
      };
    }
    return references.get(key) ?? invalid("Unknown concept endpoint");
  };
  const selectedBasis = (keys: string[]) =>
    keys.map((key) => basis.get(key) ?? invalid("Unknown concept support"));
  const revisionBasis = (keys: string[]) =>
    selectedBasis(keys).map((s) =>
      s.basis.case === "revision"
        ? s.basis.value
        : invalid("Tag lineage requires exact revision/source basis"),
    );
  const association = (key: string) =>
    associations.get(key) ?? invalid("Unknown concept association");
  const createdKeys = new Set(catalog.tags.map((t) => t.key));
  for (let index = 0; index < proposal.actions.length; index++) {
    const action = proposal.actions[index]!;
    if (action.action === "create_tag") {
      if (createdKeys.has(action.key))
        invalid("Duplicate concept temporary key");
      createdKeys.add(action.key);
    } else if (action.action === "split_tag") {
      for (const child of action.children) {
        if (createdKeys.has(child.key))
          invalid("Duplicate concept temporary key");
        createdKeys.add(child.key);
      }
    }
  }
  for (let index = results.length; index < proposal.actions.length; index++) {
    const action = proposal.actions[index]!;
    const operationId = maintenanceActionOperationId(
      operation,
      index,
      action.action,
    );
    let result: ConceptActionResult = {
      index,
      status: "committed",
      resultRef: null,
      tagResults: [],
    };
    const saveTag = (
      key: string,
      value: { tagId: string; currentRevisionId: string },
    ) => {
      result.tagResults.push({
        key,
        tagId: value.tagId,
        revisionId: value.currentRevisionId,
      });
      result.resultRef = { kind: "tag", value: value.tagId };
    };
    const content = (value: {
      label: string;
      description: string | null;
      kind_hint: string | null;
    }) => ({
      label: value.label,
      description: value.description ?? undefined,
      kindHint: value.kind_hint ?? undefined,
    });
    const createAssociation = async (
      fromKey: string,
      toKey: string,
      relation: string,
      keys: string[],
    ) => {
      const value = await mutate(operationId, () =>
        kernel.concepts.createAssociation(
          {
            operationId,
            subjectId: plan.subjectId,
            producer,
            association: {
              from: endpoint(fromKey, index),
              to: endpoint(toKey, index),
              relationKind: relation,
              polarity: "positive",
              basisClass: "cognitive_derivation",
              basis: selectedBasis(keys),
            },
          },
          options,
        ),
      );
      result.resultRef = { kind: "association", value: value.associationId };
    };
    try {
      switch (action.action) {
        case "no_change":
          result.status = "no_change";
          break;
        case "reuse_tag": {
          const target = tag(action.tagKey, index);
          result.status = "no_change";
          result.resultRef = { kind: "tag", value: target.tagId };
          break;
        }
        case "create_tag": {
          selectedBasis(action.basisKeys);
          for (const key of action.cognitionKeys) {
            if (
              !catalog.references.some(
                (r) => r.key === key && r.reference?.kind.endsWith("_revision"),
              )
            )
              invalid("Unknown concept cognition key");
          }
          saveTag(
            action.key,
            await mutate(operationId, () =>
              kernel.concepts.createTag(
                {
                  operationId,
                  subjectId: plan.subjectId,
                  producer,
                  tag: {
                    ...content(action.content),
                    origin: "concept_maintenance",
                  },
                },
                options,
              ),
            ),
          );
          break;
        }
        case "revise_tag":
          selectedBasis(action.basisKeys);
          saveTag(
            action.tagKey,
            await mutate(operationId, () =>
              kernel.concepts.reviseTag(
                {
                  operationId,
                  subjectId: plan.subjectId,
                  producer,
                  target: tag(action.tagKey, index),
                  content: content(action.content),
                },
                options,
              ),
            ),
          );
          break;
        case "attach_tag":
          await createAssociation(
            action.cognitionKey,
            action.tagKey,
            "tag_attachment",
            action.basisKeys,
          );
          break;
        case "create_association":
          await createAssociation(
            action.fromKey,
            action.toKey,
            action.relation,
            action.basisKeys,
          );
          break;
        case "detach_tag":
        case "revoke_association": {
          const target = association(action.associationKey);
          selectedBasis(action.basisKeys);
          if (
            action.action === "detach_tag" &&
            target.relation !== "tag_attachment"
          )
            invalid("Detach requires an existing Tag attachment");
          await mutate(operationId, () =>
            kernel.concepts.revokeAssociation(
              {
                operationId,
                subjectId: plan.subjectId,
                associationId: target.associationId,
              },
              options,
            ),
          );
          result.resultRef = {
            kind: "association",
            value: target.associationId,
          };
          break;
        }
        case "merge_tags": {
          const value = await mutate(operationId, () =>
            kernel.concepts.mergeTags(
              {
                operationId,
                subjectId: plan.subjectId,
                survivor: tag(action.survivorKey, index),
                retired: action.retiredKeys.map((key) => tag(key, index)),
                basis: revisionBasis(action.basisKeys),
              },
              options,
            ),
          );
          for (const key of [action.survivorKey, ...action.retiredKeys])
            saveTag(key, value);
          break;
        }
        case "split_tag": {
          const value = await mutate(operationId, () =>
            kernel.concepts.splitTag(
              {
                operationId,
                subjectId: plan.subjectId,
                producer,
                parent: tag(action.tagKey, index),
                children: action.children.map((child) =>
                  content(child.content),
                ),
                basis: revisionBasis(action.basisKeys),
              },
              options,
            ),
          );
          if (value.children.length !== action.children.length)
            throw new ConnectError(
              "Owner split result cardinality mismatch",
              Code.Internal,
            );
          for (const [childIndex, child] of value.children.entries())
            saveTag(action.children[childIndex]!.key, child);
          break;
        }
      }
    } catch (error) {
      if (error instanceof MissingConceptDependency)
        result.status = "skipped_dependency";
      else {
        const rejection = authorityRejection(error);
        if (!rejection) throw error;
        result.status = rejection;
      }
      result.resultRef = null;
      result.tagResults = [];
    }
    results.push(result);
    applyTags(result);
    await saveProgress(results);
  }
  const committed = results.some((r) => r.status === "committed");
  const failed = results.some(
    (r) => !["committed", "no_change"].includes(r.status),
  );
  const status = committed
    ? failed
      ? ("partial" as const)
      : ("committed" as const)
    : failed
      ? results.some((r) => r.status === "stale")
        ? ("stale" as const)
        : ("rejected_invalid" as const)
      : ("no_change" as const);
  return { status, actions: results };
}

// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import type {
  CognitiveRef,
  ProducerSignature,
  RevisionBasis,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import type {
  SocialContent,
  SocialRecord,
} from "@nous-wave/protocol/nous/wave/v1alpha1/social_pb.js";
import type { KernelClient } from "../kernel-client.js";
import { operationIdentity } from "../operation-identity.js";

/** Shared subject_reflection owns model execution and persists its paid result first. */
export type SocialProposal =
  | { action: "keep"; reason: string }
  | { action: "adopt"; content: SocialContent; reason: string }
  | {
      action: "revise";
      targetRevision: CognitiveRef;
      expectedObjectEpoch: bigint;
      content: SocialContent;
      reason: string;
    }
  | {
      action: "add_evidence";
      targetRevision: CognitiveRef;
      expectedObjectEpoch: bigint;
      role: "support" | "counterexample" | "boundary_case";
      basis: RevisionBasis;
      reason: string;
    };
export type SocialApplyOutcome = {
  index: number;
  action: SocialProposal["action"];
  status: "applied" | "kept" | "rejected";
  reason: string;
  record?: SocialRecord;
  error?: string;
};

/** Typed owner application only. Shared reflection must map prepared source/target keys
 * to these exact refs before calling. This does not run a model or authorize lifecycle/purge. */
export async function applySocialProposals(
  kernel: KernelClient,
  subjectId: string,
  operationId: string,
  producer: ProducerSignature,
  proposals: readonly SocialProposal[],
): Promise<SocialApplyOutcome[]> {
  if (proposals.length > 128)
    throw new Error("Shared reflection Social proposal budget exceeds 128");
  if (producer.operation !== "subject_reflection_text")
    throw new Error(
      "Social formation requires shared subject_reflection provenance",
    );
  const outcomes: SocialApplyOutcome[] = [];
  for (const [index, proposal] of proposals.entries()) {
    if (!proposal.reason.trim()) {
      outcomes.push({
        index,
        action: proposal.action,
        status: "rejected",
        reason: proposal.reason,
        error: "Social proposal requires its actual reason",
      });
      continue;
    }
    if (proposal.action === "keep") {
      outcomes.push({
        index,
        action: proposal.action,
        status: "kept",
        reason: proposal.reason,
      });
      continue;
    }
    const mutation = {
      subjectId,
      operationId: operationIdentity(
        `social:${operationId}:${index}:${proposal.action}`,
      ),
    };
    try {
      let record: SocialRecord;
      if (proposal.action === "add_evidence")
        record = await kernel.social.addEvidence({
          ...mutation,
          targetRevision: proposal.targetRevision,
          expectedObjectEpoch: proposal.expectedObjectEpoch,
          role: proposal.role,
          basis: proposal.basis,
        });
      else {
        if (!proposal.content.content.case)
          throw new Error("Social proposal lacks typed content");
        const content = {
          ...proposal.content,
          content: {
            ...proposal.content.content,
            value: { ...proposal.content.content.value, producer },
          },
        } as SocialContent;
        record =
          proposal.action === "adopt"
            ? await kernel.social.adopt({
                ...mutation,
                content,
                reason: proposal.reason,
              })
            : await kernel.social.revise({
                ...mutation,
                targetRevision: proposal.targetRevision,
                expectedObjectEpoch: proposal.expectedObjectEpoch,
                content,
                reason: proposal.reason,
              });
      }
      outcomes.push({
        index,
        action: proposal.action,
        status: "applied",
        reason: proposal.reason,
        record,
      });
    } catch (error) {
      outcomes.push({
        index,
        action: proposal.action,
        status: "rejected",
        reason: proposal.reason,
        error: error instanceof Error ? error.message : String(error),
      });
    }
  }
  return outcomes;
}

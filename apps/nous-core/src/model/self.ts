// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { create, fromJson, toJson, type JsonValue } from "@bufbuild/protobuf";
import { type ProducerSignature } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import type { SelfInterpretationProposal } from "./schemas/self.js";
import {
  InterpretSelfResponseSchema,
  type InterpretSelfRequest,
} from "@nous-wave/protocol/nous/wave/v1alpha1/model_pb.js";
import {
  AdoptSelfRequestSchema,
  ReviseSelfRequestSchema,
  AddSelfEvidenceRequestSchema,
  SelfRecordSchema,
  type SelfRecord,
} from "@nous-wave/protocol/nous/wave/v1alpha1/self_pb.js";
import type { KernelClient } from "../kernel-client.js";
import type { ModelRuntime } from "./runtime.js";
import { type ExecutionOptions } from "../execution.js";
import { executeSubjectInterpretation } from "./subject-reflection.js";
import { operationIdentity } from "../operation-identity.js";
import {
  proposedSelfContent,
  selectedSelfBasis,
  selectedSelfTarget,
  invalidSelf,
  type SelfInterpretationPlan,
} from "./self-plan.js";

export type PreparedAction = {
  action: "adopt" | "revise" | "evidence" | "keep";
  request?: JsonValue;
  record?: JsonValue;
  reason: string;
  operationId: string;
};
function actionIdentity(
  request: InterpretSelfRequest,
  index: number,
  action: string,
) {
  return operationIdentity(`self:${request.operationId}:${index}:${action}`);
}
export function compileSelfProposal(
  output: SelfInterpretationProposal,
  plan: SelfInterpretationPlan,
  producer: ProducerSignature,
  request: InterpretSelfRequest,
  operationIdentityForAction?: (
    action: SelfInterpretationProposal["actions"][number],
    index: number,
  ) => string,
) {
  const touched = new Set<string>();
  const actions: PreparedAction[] = output.actions.map((action, index) => {
    const operationId =
      operationIdentityForAction?.(action, index) ??
      actionIdentity(request, index, action.action);
    const base = {
      action: action.action,
      reason: action.reason,
      operationId,
    };
    const target =
      "targetKey" in action && action.targetKey !== null
        ? selectedSelfTarget(plan, action.targetKey)
        : undefined;
    if (action.action === "keep")
      return {
        ...base,
        action: "keep",
        record: target ? toJson(SelfRecordSchema, target) : undefined,
      };
    if ("targetKey" in action) {
      if (touched.has(action.targetKey))
        invalidSelf("A Self proposal cannot mutate one target twice");
      touched.add(action.targetKey);
    }
    if (action.action === "evidence")
      return {
        ...base,
        action: "evidence",
        request: toJson(
          AddSelfEvidenceRequestSchema,
          create(AddSelfEvidenceRequestSchema, {
            subjectId: request.subjectId,
            operationId,
            targetRevision: target!.reference,
            expectedObjectEpoch: target!.objectEpoch,
            role: action.role,
            basis: selectedSelfBasis(
              plan,
              [action.basisKey],
              action.targetKey,
            )[0],
          }),
        ),
      };
    const content = proposedSelfContent(
      plan,
      request,
      action.content,
      producer,
      "targetKey" in action ? action.targetKey : undefined,
    );
    if (action.action === "adopt")
      return {
        ...base,
        action: "adopt",
        request: toJson(
          AdoptSelfRequestSchema,
          create(AdoptSelfRequestSchema, {
            subjectId: request.subjectId,
            operationId,
            content,
            reason: action.reason,
          }),
        ),
      };
    return {
      ...base,
      action: "revise",
      request: toJson(
        ReviseSelfRequestSchema,
        create(ReviseSelfRequestSchema, {
          subjectId: request.subjectId,
          operationId,
          content,
          reason: action.reason,
          targetRevision: target!.reference,
          expectedObjectEpoch: target!.objectEpoch,
        }),
      ),
    };
  });
  return actions;
}
export async function interpretSelf(
  kernel: KernelClient,
  models: ModelRuntime,
  request: InterpretSelfRequest,
  options: ExecutionOptions,
) {
  if (!request.source.case)
    invalidSelf("Self interpretation source is required");
  const reflected = await executeSubjectInterpretation(
    kernel,
    models,
    {
      subjectId: request.subjectId,
      operationId: request.operationId,
      explicitSelf: request,
    },
    options,
  );
  const response = create(InterpretSelfResponseSchema, {
    domain: "self",
    status:
      reflected.status === "blocked_dependency" ? "failed" : reflected.status,
    degradation: reflected.degradation.map((d) => ({
      ...d,
      code:
        d.code === "reflection_proposal_rejected"
          ? "self_proposal_rejected"
          : d.code,
    })),
  });
  for (const action of reflected.actions) {
    const record =
      action.ownerResult.case === "selfRecord"
        ? action.ownerResult.value
        : undefined;
    response.actions.push({
      $typeName: "nous.wave.v1alpha1.SelfActionResult",
      action: action.action,
      status: action.status,
      record,
      reason: action.reason,
      errorCode: action.problemCode,
    });
  }
  return response;
}

/** Same owner application used by explicit Self interpretation and shared reflection. */
export async function applySelfPreparedAction(
  kernel: KernelClient,
  action: PreparedAction,
  options: ExecutionOptions,
): Promise<SelfRecord | undefined> {
  return action.action === "keep"
    ? action.record
      ? fromJson(SelfRecordSchema, action.record)
      : undefined
    : action.action === "adopt"
      ? await kernel.self.adopt(
          fromJson(AdoptSelfRequestSchema, action.request!),
          options,
        )
      : action.action === "revise"
        ? await kernel.self.revise(
            fromJson(ReviseSelfRequestSchema, action.request!),
            options,
          )
        : await kernel.self.addEvidence(
            fromJson(AddSelfEvidenceRequestSchema, action.request!),
            options,
          );
}

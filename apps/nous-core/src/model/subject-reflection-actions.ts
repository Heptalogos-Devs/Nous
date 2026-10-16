import {
  AdoptMotivationRequestSchema,
  ReviseMotivationRequestSchema,
  AddMotivationEvidenceRequestSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/motivation_pb.js";
import { chosenBasis } from "./reflection-catalog.js";
import { domainTemporal } from "./temporal.js";
import { compileMotivationAction } from "./motivation.js";
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { create, fromJson, toJson, type JsonValue } from "@bufbuild/protobuf";
import { Code, ConnectError } from "@connectrpc/connect";
import {
  FormMemoryRequestSchema,
  ReviseMemoryRequestSchema,
  MemorySchema,
  type ProducerSignature,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { InterpretSelfRequestSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/model_pb.js";
import {
  SelfRecordSchema,
  SelfScopeSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/self_pb.js";
import {
  SocialContentSchema,
  SocialRecordSchema,
  SocialScopeSchema,
  AdoptSocialRequestSchema,
  ReviseSocialRequestSchema,
  AddSocialEvidenceRequestSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/social_pb.js";
import type { SubjectReflectionProposal } from "./schemas/subject-reflection.js";
import { canonicalDigest } from "../digest.js";
import { operationIdentity } from "../operation-identity.js";
import type { ReflectionPlan } from "./subject-reflection-plan.js";
import { restoreSelfPlan } from "./subject-reflection-plan.js";
import {
  compileSelfProposal,
  applySelfPreparedAction,
  type PreparedAction,
} from "./self.js";
import type { KernelClient } from "../kernel-client.js";
import type { ExecutionOptions } from "../execution.js";
import {
  ReflectionActionOutcomeSchema,
  type ReflectionActionOutcome,
  type ReflectionRepairTarget,
} from "@nous-wave/protocol/nous/wave/v1alpha1/reflection_pb.js";
export type CompiledReflectionAction = {
  id: string;
  domain: string;
  action: string;
  operationId: string;
  reason: string;
  request?: JsonValue;
  record?: JsonValue;
  targetObject?: { kind: string; value: string };
};
function invalid(message: string): never {
  throw new ConnectError(message, Code.InvalidArgument);
}
export function reflectionActionId(
  operation: string,
  action: SubjectReflectionProposal["actions"][number],
) {
  return operationIdentity(
    `subject-reflection:${operation}:${canonicalDigest(action)}`,
  );
}
export function compileReflectionActions(
  plan: ReflectionPlan,
  output: SubjectReflectionProposal,
  producer: ProducerSignature,
  operation: string,
  identityOutput: SubjectReflectionProposal = output,
): CompiledReflectionAction[] {
  if (identityOutput.actions.length !== output.actions.length)
    invalid("Prepared reflection actions must preserve paid action identities");
  if (output.actions.length > plan.maxActions)
    invalid("Reflection proposal exceeds captured action budget");
  const touched = new Set<string>(),
    ids = new Set<string>();
  return output.actions.map((typed, index) => {
    const identity = identityOutput.actions[index]!;
    if (
      identity.domain !== typed.domain ||
      identity.proposal.action !== typed.proposal.action
    )
      invalid("Prepared reflection cannot change paid action kind or domain");
    if (!plan.domains.includes(typed.domain))
      invalid("Reflection proposal targets a domain not explicitly enabled");
    const action = typed.proposal,
      id = reflectionActionId(operation, identity);
    if (ids.has(id)) invalid("Duplicate reflection action identity");
    ids.add(id);
    const base = {
      id,
      domain: typed.domain,
      action: action.action,
      operationId: id,
      reason: action.reason,
    };
    const targetKey = "targetKey" in action ? action.targetKey : undefined;
    if (action.action !== "keep" && targetKey) {
      if (touched.has(targetKey))
        invalid("Reflection cannot mutate one exact target twice");
      touched.add(targetKey);
    }
    const scoped =
      typed.scopeKey === null
        ? undefined
        : new Map(plan.scopes).get(typed.scopeKey);
    if (typed.scopeKey !== null && !scoped)
      invalid("Reflection scope key is outside actual captured scopes");
    if (typed.domain === "motivation")
      return {
        ...base,
        ...compileMotivationAction(
          plan,
          typed.proposal,
          scoped?.social,
          id,
          producer,
        ),
      };
    if (typed.domain === "self") {
      const selfPlan = restoreSelfPlan(plan);
      const request = create(InterpretSelfRequestSchema, {
        subjectId: plan.subjectId,
        operationId: operation,
        scope: scoped ? fromJson(SelfScopeSchema, scoped.self) : undefined,
      });
      const item = compileSelfProposal(
        { actions: [typed.proposal] },
        selfPlan,
        producer,
        request,
        () => id,
      )[0]!;
      const target = targetKey ? selfPlan.current.get(targetKey) : undefined;
      return {
        ...base,
        request: item.request,
        record: item.record,
        targetObject: target?.objectRef
          ? { kind: target.objectRef.kind, value: target.objectRef.value }
          : undefined,
      };
    }
    if (typed.domain === "social") {
      const current = new Map(plan.currentSocial),
        target = targetKey
          ? fromJson(
              SocialRecordSchema,
              current.get(targetKey) ??
                invalid("Social target is outside supplied current catalog"),
            )
          : undefined;
      if (action.action === "keep")
        return {
          ...base,
          record: target ? toJson(SocialRecordSchema, target) : undefined,
        };
      if (!scoped) invalid("Social mutation requires an actual scope key");
      const mutation = {
        subjectId: plan.subjectId,
        operationId: id,
        targetRevision: target?.reference,
        expectedObjectEpoch: target?.objectEpoch,
      };
      if (typed.proposal.action === "add_evidence")
        return {
          ...base,
          request: toJson(
            AddSocialEvidenceRequestSchema,
            create(AddSocialEvidenceRequestSchema, {
              ...mutation,
              role: typed.proposal.role,
              basis: chosenBasis(
                plan,
                [typed.proposal.basisKey],
                targetKey ?? undefined,
              )[0],
            }),
          ),
          targetObject: target?.objectRef,
        };
      const proposal = typed.proposal;
      if (proposal.action === "keep") return base;
      const c = proposal.content;
      const entity = (key: string) =>
        new Map(plan.entities).get(key) ??
        invalid("Social entity key is not an actually bound Host entity");
      const common = {
        scope: fromJson(SocialScopeSchema, scoped.social),
        judgment: {
          state: c.judgment.state,
          explanation: c.judgment.explanation,
          estimate: c.judgment.estimate ?? undefined,
          estimateMethod: c.judgment.estimateMethod ?? undefined,
        },
        basis: chosenBasis(plan, c.basisKeys, targetKey ?? undefined),
        validTime: domainTemporal(c.validTime),
        producer,
      };
      const content = create(SocialContentSchema, {
        content:
          c.family === "social_relation"
            ? {
                case: "relation",
                value: {
                  ...common,
                  from: entity(c.fromKey),
                  to: entity(c.toKey),
                  relationType: c.relationType,
                  degree: c.degree
                    ? {
                        degree:
                          c.degree.kind === "affinity"
                            ? { case: "affinity", value: c.degree.value }
                            : c.degree.kind === "ordinal"
                              ? { case: "ordinal", value: c.degree.value }
                              : { case: "state", value: c.degree.value },
                      }
                    : undefined,
                  role: c.role ?? undefined,
                  symmetric: c.symmetric,
                  statement: c.statement,
                  coordinationExpectations: c.coordinationExpectations,
                },
              }
            : {
                case: "convention",
                value: {
                  ...common,
                  expression: c.expression,
                  variants: c.variants,
                  caseInsensitive: c.caseInsensitive,
                  meaning: c.meaning,
                  referent: c.referentKey ? entity(c.referentKey) : undefined,
                  pragmaticUse: c.pragmaticUse,
                },
              },
      });
      return {
        ...base,
        request:
          proposal.action === "adopt"
            ? toJson(
                AdoptSocialRequestSchema,
                create(AdoptSocialRequestSchema, {
                  ...mutation,
                  content,
                  reason: proposal.reason,
                }),
              )
            : toJson(
                ReviseSocialRequestSchema,
                create(ReviseSocialRequestSchema, {
                  ...mutation,
                  content,
                  reason: proposal.reason,
                }),
              ),
        targetObject: target?.objectRef,
      };
    }
    const proposal = typed.proposal;
    if (typed.domain !== "memory")
      return invalid("Unsupported reflection domain");
    const current = new Map(plan.currentMemory);
    const target = targetKey
      ? fromJson(
          MemorySchema,
          current.get(targetKey) ??
            invalid("Memory target is outside supplied current exact anchors"),
        )
      : undefined;
    if (proposal.action === "keep")
      return {
        ...base,
        record: target ? toJson(MemorySchema, target) : undefined,
      };
    const basis = chosenBasis(
      plan,
      proposal.content.basisKeys,
      targetKey ?? undefined,
    );
    const grounding = basis.find((b) => b.basis.case === "evidence");
    if (grounding?.basis.case !== "evidence")
      invalid("Memory formation requires actual observed Evidence");
    const content = {
      producer,
      formationMode: "grounded",
      groundingOccurrenceId: grounding.basis.value.occurrenceId,
      semanticRole: proposal.content.semanticRole,
      text: proposal.content.text,
      basis,
      aboutness: proposal.content.entityKeys.map(
        (k) =>
          new Map(plan.entities).get(k) ??
          invalid("Memory entity key is outside actual Host catalog"),
      ),
      epistemicClass: proposal.content.epistemicClass,
      validTime: domainTemporal(proposal.content.validTime),
    };
    return {
      ...base,
      request:
        proposal.action === "adopt"
          ? toJson(
              FormMemoryRequestSchema,
              create(FormMemoryRequestSchema, {
                subjectId: plan.subjectId,
                operationId: id,
                cognitiveRole: "declarative",
                input: content,
              }),
            )
          : toJson(
              ReviseMemoryRequestSchema,
              create(ReviseMemoryRequestSchema, {
                subjectId: plan.subjectId,
                operationId: id,
                memoryId: target!.memoryId,
                expectedObjectEpoch: target!.objectEpoch,
                input: content,
                intent: "reinterpret",
              }),
            ),
      targetObject: target
        ? { kind: "memory", value: target.memoryId }
        : undefined,
    };
  });
}
export async function applyReflectionActions(
  kernel: KernelClient,
  actions: CompiledReflectionAction[],
  options: ExecutionOptions,
  resolutions: ReflectionRepairTarget[] = [],
): Promise<ReflectionActionOutcome[]> {
  const targets = new Map(resolutions.map((r) => [r.actionId, r]));
  if (targets.size !== resolutions.length)
    invalid("Duplicate explicit repair action");
  for (const r of resolutions)
    if (
      !actions.some(
        (a) =>
          a.id === r.actionId &&
          a.action !== "keep" &&
          a.action !== "adopt" &&
          a.targetObject,
      )
    )
      invalid("Repair can resolve only a supplied existing mutation target");
  const outcomes: ReflectionActionOutcome[] = [];
  for (const original of actions) {
    const action = { ...original };
    try {
      const resolution = targets.get(action.id);
      if (resolution) {
        const head = resolution.exactHead;
        if (!head || resolution.expectedObjectEpoch < 1n)
          invalid("Repair requires reviewed exact head and epoch");
        let object: { kind: string; value: string } | undefined;
        if (action.domain === "self") {
          const r = await kernel.self.getRevision(
            {
              subjectId: (action.request as { subjectId: string }).subjectId,
              id: head.value,
            },
            options,
          );
          if (r.currentRevisionId !== head.value)
            invalid("Reviewed Self repair head is no longer current");
          object = r.objectRef;
        } else if (action.domain === "motivation") {
          const r = await kernel.motivation.getRevision(
            {
              subjectId: (action.request as { subjectId: string }).subjectId,
              id: head.value,
            },
            options,
          );
          if (r.currentRevisionId !== head.value)
            invalid("Reviewed Motivation repair head is no longer current");
          object = r.objectRef;
        } else if (action.domain === "social") {
          const r = await kernel.social.getRevision(
            {
              subjectId: (action.request as { subjectId: string }).subjectId,
              id: head.value,
            },
            options,
          );
          if (r.currentRevisionId !== head.value)
            invalid("Reviewed Social repair head is no longer current");
          object = r.objectRef;
        } else {
          const c = await kernel.maintenance.reflectionMemoryCandidates(
            {
              subjectId: (action.request as { subjectId: string }).subjectId,
              references: [head],
              maxItems: 1,
              maxBytes: 1048576n,
            },
            options,
          );
          const r = c.items[0];
          if (!r) invalid("Reviewed Memory repair head is no longer current");
          object = { kind: "memory", value: r.memoryId };
        }
        if (
          !object ||
          object.kind !== action.targetObject?.kind ||
          object.value !== action.targetObject?.value
        )
          invalid(
            "Reviewed repair head must belong to the original exact target object",
          );
        action.request = {
          ...(action.request as Record<string, JsonValue>),
          expectedObjectEpoch: resolution.expectedObjectEpoch.toString(),
          ...(action.domain === "memory"
            ? {}
            : { targetRevision: { kind: head.kind, value: head.value } }),
        };
      }
      let references: { kind: string; value: string }[] = [];
      let ownerResult: ReflectionActionOutcome["ownerResult"] = {
        case: undefined,
      };
      if (action.action !== "keep") {
        if (action.domain === "self") {
          const record = await applySelfPreparedAction(
            kernel,
            { ...action, action: action.action as PreparedAction["action"] },
            options,
          );
          if (record) {
            references = [record.objectRef!, record.reference!];
            ownerResult = { case: "selfRecord", value: record };
          }
        } else if (action.domain === "motivation") {
          const record =
            action.action === "adopt"
              ? await kernel.motivation.adopt(
                  fromJson(AdoptMotivationRequestSchema, action.request!),
                  options,
                )
              : action.action === "revise"
                ? await kernel.motivation.revise(
                    fromJson(ReviseMotivationRequestSchema, action.request!),
                    options,
                  )
                : await kernel.motivation.addEvidence(
                    fromJson(
                      AddMotivationEvidenceRequestSchema,
                      action.request!,
                    ),
                    options,
                  );
          references = [record.objectRef!, record.reference!];
          ownerResult = { case: "motivationRecord", value: record };
        } else if (action.domain === "social") {
          const record =
            action.action === "adopt"
              ? await kernel.social.adopt(
                  fromJson(AdoptSocialRequestSchema, action.request!),
                  options,
                )
              : action.action === "revise"
                ? await kernel.social.revise(
                    fromJson(ReviseSocialRequestSchema, action.request!),
                    options,
                  )
                : await kernel.social.addEvidence(
                    fromJson(AddSocialEvidenceRequestSchema, action.request!),
                    options,
                  );
          references = [record.objectRef!, record.reference!];
          ownerResult = { case: "socialRecord", value: record };
        } else {
          const record =
            action.action === "adopt"
              ? await kernel.memory.formMemory(
                  fromJson(FormMemoryRequestSchema, action.request!),
                  options,
                )
              : await kernel.memory.reviseMemory(
                  fromJson(ReviseMemoryRequestSchema, action.request!),
                  options,
                );
          ownerResult = { case: "memoryRecord", value: record };
          references = [
            { kind: "memory", value: record.memoryId },
            { kind: "memory_revision", value: record.revisionId },
          ];
        }
      } else if (action.record) {
        if (action.domain === "self") {
          const r = fromJson(SelfRecordSchema, action.record);
          references = [r.objectRef!, r.reference!];
          ownerResult = { case: "selfRecord", value: r };
        } else if (action.domain === "social") {
          const r = fromJson(SocialRecordSchema, action.record);
          references = [r.objectRef!, r.reference!];
          ownerResult = { case: "socialRecord", value: r };
        } else {
          const r = fromJson(MemorySchema, action.record);
          ownerResult = { case: "memoryRecord", value: r };
          references = [
            { kind: "memory", value: r.memoryId },
            { kind: "memory_revision", value: r.revisionId },
          ];
        }
      }
      outcomes.push(
        create(ReflectionActionOutcomeSchema, {
          actionId: action.id,
          domain: action.domain,
          action: action.action,
          operationId: action.operationId,
          status: action.action === "keep" ? "kept" : "applied",
          reason: action.reason,
          references,
          ownerResult,
        }),
      );
    } catch (error) {
      if (
        !(error instanceof ConnectError) ||
        options.signal?.aborted ||
        ![
          Code.InvalidArgument,
          Code.NotFound,
          Code.AlreadyExists,
          Code.FailedPrecondition,
          Code.Aborted,
          Code.OutOfRange,
        ].includes(error.code)
      )
        throw error;
      outcomes.push(
        create(ReflectionActionOutcomeSchema, {
          actionId: action.id,
          domain: action.domain,
          action: action.action,
          operationId: action.operationId,
          status: "failed",
          reason: error.rawMessage,
          problemCode: Code[error.code],
        }),
      );
    }
  }
  return outcomes;
}

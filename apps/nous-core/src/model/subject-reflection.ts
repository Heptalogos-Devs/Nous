// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { create, fromJson, toJson, type JsonValue } from "@bufbuild/protobuf";
import { DomainStampSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { Code, ConnectError } from "@connectrpc/connect";
import { z } from "zod";
import { ProducerSignatureSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import {
  InterpretSelfRequestSchema,
  type InterpretSelfRequest,
} from "@nous-wave/protocol/nous/wave/v1alpha1/model_pb.js";
import {
  ReflectionOutcomeSchema,
  type ReflectionOutcome,
  type ReflectionRepairRequest,
} from "@nous-wave/protocol/nous/wave/v1alpha1/reflection_pb.js";
import {
  SubjectReflectionBatchSchema,
  type MaintenanceNeed,
} from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/longitudinal_pb.js";
import { type KernelClient } from "../kernel-client.js";
import { type ModelRuntime } from "./runtime.js";
import { executionOptions, type ExecutionOptions } from "../execution.js";
import { canonicalDigest } from "../digest.js";
import {
  runRecordedOperation,
  serializeOperation,
  type Dependency,
} from "../operation-results.js";
import { maintenanceOperationId } from "../maintenance/identity.js";
import {
  GenerationFailure,
  attachExecutionTelemetry,
} from "./execution/routes.js";
import { modelProducer } from "./producer.js";
import {
  prepareExplicitSelfPlan,
  prepareReflectionPlan,
  type ReflectionPlan,
} from "./subject-reflection-plan.js";
import {
  compileReflectionActions,
  applyReflectionActions,
  reflectionActionId,
  type CompiledReflectionAction,
} from "./subject-reflection-actions.js";
import {
  subjectReflectionSchema,
  type SubjectReflectionProposal,
} from "./schemas/subject-reflection.js";
import type { SelfInterpretationProposal } from "./schemas/self.js";
export type SavedReflectionProposal = {
  plan: ReflectionPlan;
  workId: string;
  output: SubjectReflectionProposal;
  preparation: SubjectReflectionProposal;
  producer: JsonValue;
  formedAt?: JsonValue;
};
type SavedReflectionOutcome = {
  result: JsonValue;
  proposal?: SavedReflectionProposal;
};
function readOutcome(saved: {
  purged: boolean;
  payloadJson: string;
}): ReflectionOutcome {
  if (saved.purged)
    throw new ConnectError(
      "Reflection result or source was purged",
      Code.NotFound,
    );
  const data = JSON.parse(saved.payloadJson) as SavedReflectionOutcome;
  return fromJson(ReflectionOutcomeSchema, data.result);
}
function outcomeStatus(actions: { status: string }[]) {
  const failed = actions.filter((a) => a.status === "failed").length;
  return failed === actions.length
    ? "failed"
    : failed
      ? "partial"
      : actions.every((a) => a.status === "kept")
        ? "kept"
        : "applied";
}
export async function runSubjectReflection(
  kernel: KernelClient,
  models: ModelRuntime,
  need: MaintenanceNeed,
  options: ExecutionOptions,
  modelBudget: number,
  reserveModelCall: () => void,
) {
  return executeSubjectInterpretation(
    kernel,
    models,
    {
      subjectId: need.subjectId,
      operationId: maintenanceOperationId(need),
      selected: need,
      modelBudget,
      reserveModelCall,
    },
    options,
  );
}
export async function executeSubjectInterpretation(
  kernel: KernelClient,
  models: ModelRuntime,
  input: {
    subjectId: string;
    operationId: string;
    selected?: MaintenanceNeed;
    explicitSelf?: InterpretSelfRequest;
    modelBudget?: number;
    reserveModelCall?: () => void;
  },
  options: ExecutionOptions,
): Promise<ReflectionOutcome> {
  if (
    !z.uuid().safeParse(input.subjectId).success ||
    !z.uuid().safeParse(input.operationId).success
  )
    throw new ConnectError(
      "Reflection requires canonical Subject and work operation UUIDs",
      Code.InvalidArgument,
    );
  const calls = executionOptions(kernel.execution.opportunity, options);
  const role = input.explicitSelf
    ? input.explicitSelf.source.case === "seedVersionId"
      ? "self_formation"
      : "self_reflection"
    : "subject_reflection";
  const identity = {
    subjectId: input.subjectId,
    owner: "subject_reflection",
    operationKey: input.operationId,
    semanticDigest: canonicalDigest(
      input.explicitSelf
        ? {
            explicitSelf: toJson(
              InterpretSelfRequestSchema,
              input.explicitSelf,
            ),
          }
        : {
            needId: input.selected!.needId,
            trigger: input.selected!.triggerAuthoritySeq.toString(),
            triggerRevision: input.selected!.triggerRevision.toString(),
          },
    ),
  };
  return serializeOperation(
    kernel,
    `subject-reflection:${input.subjectId}:${input.operationId}`,
    calls.signal,
    () =>
      runRecordedOperation<SavedReflectionProposal, ReflectionOutcome>(
        kernel,
        identity,
        calls,
        {
          context: input.selected
            ? {
                maintenanceTrigger: {
                  needId: input.selected.needId,
                  triggerAuthoritySeq: input.selected.triggerAuthoritySeq,
                  triggerRevision: input.selected.triggerRevision,
                },
              }
            : undefined,
          replay: readOutcome,
          generate: async () => {
            if (input.modelBudget === 0)
              return {
                value: create(ReflectionOutcomeSchema, {
                  operationId: input.operationId,
                  status: "deferred",
                  problemCode: "model_budget_exhausted",
                }),
              };
            let prepared;
            try {
              prepared = models.invocations.prepare(role);
            } catch (error) {
              models.invocations.allowDegradation(error, calls.signal, role);
              return {
                value: create(ReflectionOutcomeSchema, {
                  operationId: input.operationId,
                  status: "blocked_dependency",
                  problemCode: "model_unavailable",
                }),
              };
            }
            const batch = input.selected
              ? await kernel.maintenance.prepareSubjectReflection(
                  { selected: input.selected },
                  calls,
                )
              : undefined;
            if (batch && (!batch.domains.length || !batch.sources.length))
              return {
                value: create(ReflectionOutcomeSchema, {
                  operationId: input.operationId,
                  status: batch.changes.length
                    ? "blocked_dependency"
                    : "obsolete",
                  problemCode: batch.domains.length
                    ? batch.changes.length
                      ? "source_change_requires_actual_evidence"
                      : "no_actual_sources"
                    : "no_enabled_domains",
                }),
              };
            let plan: ReflectionPlan;
            try {
              plan = input.explicitSelf
                ? await prepareExplicitSelfPlan(
                    kernel,
                    input.explicitSelf,
                    models.configuration.material_inputs
                      .formation_source_max_bytes,
                    calls,
                  )
                : await prepareReflectionPlan(kernel, batch!, calls);
            } catch (error) {
              if (!batch || !(error instanceof ConnectError)) throw error;
              const problem = `reflection_preparation_${Code[error.code]}: ${error.rawMessage.slice(0, 512)}`;
              const marked = await kernel.maintenance.finishSubjectReflection(
                {
                  selected: batch.selected,
                  throughSequence: batch.fromSequence,
                  throughChangeId: 0n,
                  operationId: input.operationId,
                  status: "blocked",
                  problemCode: problem,
                },
                calls.opportunity!.acknowledge(),
              );
              return {
                value: create(ReflectionOutcomeSchema, {
                  operationId: input.operationId,
                  status: "blocked_dependency",
                  problemCode: problem,
                  operationsConsumed: marked.operationsConsumed,
                }),
              };
            }
            const generated = await models.invocations.generate(
              role,
              { content: plan.input },
              {
                signal: calls.signal,
                prepared,
                beforeAttempt: input.reserveModelCall,
              },
            );
            try {
              const output: SubjectReflectionProposal = input.explicitSelf
                ? {
                    actions: (
                      generated.value as SelfInterpretationProposal
                    ).actions.map((proposal) => ({
                      domain: "self" as const,
                      scopeKey: "invocation",
                      proposal,
                    })),
                  }
                : subjectReflectionSchema.parse(generated.value);
              const producer = modelProducer(
                generated.producerMetadata,
                role === "self_formation"
                  ? "self_formation_text"
                  : role === "self_reflection"
                    ? "self_reflection_text"
                    : "subject_reflection_text",
              );
              const clock = await kernel.clocks.readClock(
                { subjectId: input.subjectId },
                calls,
              );
              return {
                proposal: {
                  plan,
                  workId: input.operationId,
                  output,
                  preparation: output,
                  producer: toJson(ProducerSignatureSchema, producer),
                  formedAt: toJson(DomainStampSchema, clock.now!),
                },
                context: {
                  dependencies: plan.dependencies,
                  cognitiveFormedAt: clock.now,
                  maintenanceTrigger: input.selected
                    ? {
                        needId: input.selected.needId,
                        triggerAuthoritySeq: input.selected.triggerAuthoritySeq,
                        triggerRevision: input.selected.triggerRevision,
                      }
                    : undefined,
                },
                dependencies: plan.dependencies,
                execution: generated.execution,
                mutationOperations: output.actions
                  .filter((a) => a.proposal.action !== "keep")
                  .map((a) => reflectionActionId(input.operationId, a)),
              };
            } catch (error) {
              throw attachExecutionTelemetry(error, generated.execution);
            }
          },
          commit: async (proposal) => {
            const result = create(ReflectionOutcomeSchema, {
              operationId: input.operationId,
              status: "applied",
              degradation: proposal.plan.degradation,
            });
            let compiled: CompiledReflectionAction[];
            try {
              compiled = compileReflectionActions(
                proposal.plan,
                proposal.preparation,
                fromJson(ProducerSignatureSchema, proposal.producer),
                input.operationId,
                proposal.output,
              );
            } catch (error) {
              if (
                !(error instanceof ConnectError) ||
                error.code !== Code.InvalidArgument
              )
                throw error;
              result.status = "failed";
              result.problemCode = "proposal_invalid";
              result.degradation.push({
                $typeName: "nous.wave.v1alpha1.Degradation",
                code: "reflection_proposal_rejected",
                detail: error.rawMessage,
              });
              compiled = [];
            }
            if (compiled.length) {
              result.actions = await applyReflectionActions(
                kernel,
                compiled,
                calls,
              );
              result.status = outcomeStatus(result.actions);
            }
            if (proposal.plan.batch) {
              const batch = fromJson(
                SubjectReflectionBatchSchema,
                proposal.plan.batch,
              );
              result.throughSequence = batch.throughSequence;
              const completed =
                await kernel.maintenance.finishSubjectReflection(
                  {
                    selected: batch.selected,
                    throughSequence: batch.throughSequence,
                    throughChangeId: batch.throughChangeId,
                    operationId: input.operationId,
                    status: result.status,
                    problemCode:
                      result.status === "partial" || result.status === "failed"
                        ? "reflection_action_rejected"
                        : undefined,
                  },
                  calls.opportunity!.acknowledge(),
                );
              result.operationsConsumed = completed.operationsConsumed;
            }
            return {
              value: result,
              outcome: {
                result: toJson(ReflectionOutcomeSchema, result),
                proposal,
              } satisfies SavedReflectionOutcome,
              dependencies: result.actions.flatMap(
                (a) => a.references,
              ) as Dependency[],
            };
          },
          reject: async (error) => {
            if (
              !input.explicitSelf ||
              !(error instanceof GenerationFailure) ||
              calls.signal?.aborted
            )
              return undefined;
            models.invocations.allowDegradation(error, calls.signal, role);
            const result = create(ReflectionOutcomeSchema, {
              operationId: input.operationId,
              status: "failed",
              problemCode: `${role}_failed`,
              degradation: [{ code: `${role}_failed`, detail: error.message }],
            });
            return {
              value: result,
              outcome: {
                result: toJson(ReflectionOutcomeSchema, result),
              } satisfies SavedReflectionOutcome,
            };
          },
        },
      ),
  );
}
export async function repairSubjectReflection(
  kernel: KernelClient,
  request: ReflectionRepairRequest,
  options: ExecutionOptions,
): Promise<ReflectionOutcome> {
  if (
    !z.uuid().safeParse(request.repairId).success ||
    !request.resolutions.length ||
    request.resolutions.length > 128
  )
    throw new ConnectError(
      "Repair requires a canonical review operation and explicit exact-head resolutions",
      Code.InvalidArgument,
    );
  const original = await kernel.modelResults.readOperation(
    {
      subjectId: request.subjectId,
      owner: "subject_reflection",
      operationKey: request.operationId,
      semanticDigest: request.semanticDigest,
    },
    options,
  );
  if (!original.outcome || original.outcome.purged)
    throw new ConnectError(
      "A retained paid partial outcome is required for reviewed repair",
      Code.FailedPrecondition,
    );
  const saved = JSON.parse(
    original.outcome.payloadJson,
  ) as SavedReflectionOutcome;
  const proposal = saved.proposal;
  if (!proposal)
    throw new ConnectError(
      "Original reflection has no retained paid proposal",
      Code.FailedPrecondition,
    );
  const previous = fromJson(ReflectionOutcomeSchema, saved.result);
  const failed = new Set(
    previous.actions
      .filter((a) => a.status === "failed")
      .map((a) => a.actionId),
  );
  if (request.resolutions.some((r) => !failed.has(r.actionId)))
    throw new ConnectError(
      "Repair resolutions must select failed paid actions; committed actions are immutable receipts",
      Code.InvalidArgument,
    );
  if (request.resolutions.length !== failed.size)
    throw new ConnectError(
      "Reviewed repair must cover every failed action from the retained outcome",
      Code.InvalidArgument,
    );
  const identity = {
    subjectId: request.subjectId,
    owner: "subject_reflection",
    operationKey: request.repairId,
    semanticDigest: canonicalDigest({
      parent: request.operationId,
      sourceDigest: request.semanticDigest,
      resolutions: request.resolutions.map((r) => ({
        action: r.actionId,
        head: r.exactHead
          ? { kind: r.exactHead.kind, value: r.exactHead.value }
          : null,
        epoch: r.expectedObjectEpoch.toString(),
      })),
    }),
  };
  const calls = executionOptions(kernel.execution.opportunity, options);
  return serializeOperation(
    kernel,
    `subject-reflection:${request.subjectId}:${request.repairId}`,
    calls.signal,
    () =>
      runRecordedOperation<
        { actions: CompiledReflectionAction[]; paid: SavedReflectionProposal },
        ReflectionOutcome
      >(kernel, identity, calls, {
        replay: readOutcome,
        generate: async () => {
          const wanted = new Set(request.resolutions.map((r) => r.actionId));
          const actions = compileReflectionActions(
            proposal.plan,
            proposal.preparation,
            fromJson(ProducerSignatureSchema, proposal.producer),
            proposal.workId ?? request.operationId,
            proposal.output,
          ).filter((a) => wanted.has(a.id));
          return {
            proposal: { actions, paid: proposal },
            context: {
              dependencies: proposal.plan.dependencies,
              cognitiveFormedAt: proposal.formedAt
                ? fromJson(DomainStampSchema, proposal.formedAt)
                : undefined,
            },
            dependencies: proposal.plan.dependencies,
            mutationOperations: actions.map((a) => a.operationId),
          };
        },
        commit: async (p) => {
          const actions = await applyReflectionActions(
            kernel,
            p.actions,
            calls,
            request.resolutions,
          );
          const result = create(ReflectionOutcomeSchema, {
            operationId: request.repairId,
            status: outcomeStatus(actions),
            actions,
            operationsConsumed: 0,
            throughSequence: previous.throughSequence,
          });
          if (
            p.paid.plan.batch &&
            ["applied", "kept"].includes(result.status)
          ) {
            const batch = fromJson(
              SubjectReflectionBatchSchema,
              p.paid.plan.batch,
            );
            const completed = await kernel.maintenance.finishSubjectReflection(
              {
                selected: batch.selected,
                throughSequence: batch.throughSequence,
                throughChangeId: batch.throughChangeId,
                operationId: request.repairId,
                status: result.status,
              },
              calls.opportunity!.acknowledge(),
            );
            result.operationsConsumed = completed.operationsConsumed;
            await kernel.maintenance.finishMaintenance(
              { selected: batch.selected, disposition: "satisfied" },
              calls.opportunity!.acknowledge(),
            );
          }
          return {
            value: result,
            outcome: {
              result: toJson(ReflectionOutcomeSchema, result),
              proposal: p.paid,
            } satisfies SavedReflectionOutcome,
            dependencies: actions.flatMap((a) => a.references),
          };
        },
      }),
  );
}

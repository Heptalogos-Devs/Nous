// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { create, fromJson, toJson, type JsonValue } from "@bufbuild/protobuf";
import {
  RevisionBasisSchema,
  ProducerSignatureSchema,
  type ProducerSignature,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import type { SelfInterpretationProposal } from "./schemas/self.js";
import { Code, ConnectError } from "@connectrpc/connect";
import { z } from "zod";
import {
  InterpretSelfRequestSchema,
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
import { executionOptions, type ExecutionOptions } from "../execution.js";
import { canonicalDigest } from "../digest.js";
import { operationIdentity } from "../operation-identity.js";
import {
  runRecordedOperation,
  serializeOperation,
  type Dependency,
} from "../operation-results.js";
import {
  GenerationFailure,
  attachExecutionTelemetry,
} from "./execution/routes.js";
import { modelProducer } from "./producer.js";
import {
  prepareSelfInterpretation,
  proposedSelfContent,
  selectedSelfBasis,
  selectedSelfTarget,
  invalidSelf,
  type SelfInterpretationPlan,
} from "./self-plan.js";

type PreparedAction = {
  action: "adopt" | "revise" | "evidence" | "keep";
  request?: JsonValue;
  record?: JsonValue;
  reason: string;
  operationId: string;
};
type SavedSelfProposal = {
  output: SelfInterpretationProposal;
  basis: [string, JsonValue][];
  current: [string, JsonValue][];
  producer: JsonValue;
};
function actionIdentity(
  request: InterpretSelfRequest,
  index: number,
  action: string,
) {
  return operationIdentity(`self:${request.operationId}:${index}:${action}`);
}
function compileSelfProposal(
  output: SelfInterpretationProposal,
  plan: SelfInterpretationPlan,
  producer: ProducerSignature,
  request: InterpretSelfRequest,
) {
  const touched = new Set<string>();
  const actions: PreparedAction[] = output.actions.map((action, index) => {
    const operationId = actionIdentity(request, index, action.action);
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
function recordDependencies(record: SelfRecord): Dependency[] {
  return [record.objectRef, record.reference].filter(
    (ref): ref is NonNullable<typeof ref> => ref !== undefined,
  );
}
export async function interpretSelf(
  kernel: KernelClient,
  models: ModelRuntime,
  request: InterpretSelfRequest,
  options: ExecutionOptions,
) {
  if (
    !z.uuid().safeParse(request.subjectId).success ||
    !z.uuid().safeParse(request.operationId).success
  )
    invalidSelf(
      "Self interpretation requires canonical Subject and operation UUIDs",
    );
  if (!request.source.case)
    invalidSelf("Self interpretation source is required");
  const calls = executionOptions(kernel.execution.opportunity, options);
  return serializeOperation(
    kernel,
    `self:${request.subjectId.toLowerCase()}:${request.operationId.toLowerCase()}`,
    calls.signal,
    () => interpretOnce(kernel, models, request, calls),
  );
}
async function interpretOnce(
  kernel: KernelClient,
  models: ModelRuntime,
  request: InterpretSelfRequest,
  options: ExecutionOptions,
) {
  const identity = {
    subjectId: request.subjectId,
    owner: "self",
    operationKey: request.operationId,
    semanticDigest: canonicalDigest(
      toJson(InterpretSelfRequestSchema, request),
    ),
  };
  const role =
    request.source.case === "seedVersionId"
      ? "self_formation"
      : "self_reflection";
  return runRecordedOperation<
    SavedSelfProposal,
    ReturnType<typeof create<typeof InterpretSelfResponseSchema>>
  >(kernel, identity, options, {
    replay: (saved) => {
      if (saved.purged)
        throw new ConnectError(
          "Self interpretation result was purged",
          Code.NotFound,
        );
      return fromJson(
        InterpretSelfResponseSchema,
        JSON.parse(saved.payloadJson) as JsonValue,
      );
    },
    generate: async () => {
      const plan = await prepareSelfInterpretation(
        kernel,
        request,
        models.configuration.material_inputs.formation_source_max_bytes,
        options,
      );
      const model = models.invocations.prepare(role);
      const generated = await models.invocations.generate(
        role,
        { content: plan.input },
        { signal: options.signal ?? undefined, prepared: model },
      );
      try {
        const producer = modelProducer(
          generated.producerMetadata,
          role === "self_formation"
            ? "self_formation_text"
            : "self_reflection_text",
        );
        const dependencies: Dependency[] = [...plan.basis.values()].flatMap(
          (entry) =>
            entry.basis.case === "cognitionDependency" &&
            entry.basis.value.targetRevision
              ? [entry.basis.value.targetRevision]
              : entry.basis.case === "seed" && entry.basis.value.seedVersion
                ? [entry.basis.value.seedVersion]
                : entry.basis.case === "evidence"
                  ? [
                      {
                        kind: "occurrence",
                        value: entry.basis.value.occurrenceId,
                      },
                    ]
                  : [],
        );
        return {
          proposal: {
            output: generated.value,
            basis: [...plan.basis].map(([key, value]) => [
              key,
              toJson(RevisionBasisSchema, value),
            ]),
            current: [...plan.current].map(([key, value]) => [
              key,
              toJson(SelfRecordSchema, value),
            ]),
            producer: toJson(ProducerSignatureSchema, producer),
          },
          dependencies,
          execution: generated.execution,
          mutationOperations: generated.value.actions.flatMap(
            (action, index) =>
              action.action === "keep"
                ? []
                : [actionIdentity(request, index, action.action)],
          ),
        };
      } catch (error) {
        throw attachExecutionTelemetry(error, generated.execution);
      }
    },
    commit: async (proposal) => {
      const response = create(InterpretSelfResponseSchema, {
        domain: "self",
        status: "applied",
      });
      let actions: PreparedAction[];
      try {
        actions = compileSelfProposal(
          proposal.output,
          {
            input: "",
            basis: new Map(
              proposal.basis.map(([key, value]) => [
                key,
                fromJson(RevisionBasisSchema, value),
              ]),
            ),
            current: new Map(
              proposal.current.map(([key, value]) => [
                key,
                fromJson(SelfRecordSchema, value),
              ]),
            ),
          },
          fromJson(ProducerSignatureSchema, proposal.producer),
          request,
        );
      } catch (error) {
        if (
          !(error instanceof ConnectError) ||
          error.code !== Code.InvalidArgument
        )
          throw error;
        response.status = "failed";
        response.degradation.push({
          $typeName: "nous.wave.v1alpha1.Degradation",
          code: "self_proposal_rejected",
          detail: error.rawMessage,
        });
        return {
          value: response,
          outcome: toJson(InterpretSelfResponseSchema, response),
        };
      }
      for (const action of actions) {
        try {
          const record =
            action.action === "keep"
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
          response.actions.push({
            $typeName: "nous.wave.v1alpha1.SelfActionResult",
            action: action.action,
            status: action.action === "keep" ? "kept" : "applied",
            record,
            reason: action.reason,
          });
        } catch (error) {
          if (
            !(error instanceof ConnectError) ||
            ![
              Code.InvalidArgument,
              Code.NotFound,
              Code.AlreadyExists,
              Code.FailedPrecondition,
              Code.Aborted,
              Code.OutOfRange,
            ].includes(error.code) ||
            options.signal?.aborted
          )
            throw error;
          response.actions.push({
            $typeName: "nous.wave.v1alpha1.SelfActionResult",
            action: action.action,
            status: "failed",
            reason: error.rawMessage,
            errorCode: Code[error.code],
          });
        }
      }
      const failed = response.actions.filter(
        (action) => action.status === "failed",
      ).length;
      response.status =
        failed === response.actions.length
          ? "failed"
          : failed > 0
            ? "partial"
            : response.actions.every((action) => action.status === "kept")
              ? "kept"
              : "applied";
      return {
        value: response,
        outcome: toJson(InterpretSelfResponseSchema, response),
        dependencies: response.actions.flatMap((action) =>
          action.record ? recordDependencies(action.record) : [],
        ),
      };
    },
    reject: async (error) => {
      if (!(error instanceof GenerationFailure) || options.signal?.aborted)
        return undefined;
      models.invocations.allowDegradation(error, options.signal, role);
      const response = create(InterpretSelfResponseSchema, {
        domain: "self",
        status: "failed",
        degradation: [{ code: `${role}_failed`, detail: error.message }],
      });
      return {
        value: response,
        outcome: toJson(InterpretSelfResponseSchema, response),
      };
    },
  });
}

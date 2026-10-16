import { ConditionEvaluationPlanSchema } from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/motivation_pb.js";
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import {
  fromJson,
  toJson,
  toBinary,
  type JsonObject,
  type JsonValue,
} from "@bufbuild/protobuf";
import { timestampDate } from "@bufbuild/protobuf/wkt";
import {
  EvaluateConditionRequestSchema,
  ConditionEvaluationSchema,
  type EvaluateConditionRequest,
} from "@nous-wave/protocol/nous/wave/v1alpha1/motivation_pb.js";
import { ProducerSignatureSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import type { KernelClient } from "../kernel-client.js";
import type { ModelRuntime } from "./runtime.js";
import { executionOptions, type ExecutionOptions } from "../execution.js";
import { canonicalDigest } from "../digest.js";
import {
  runRecordedOperation,
  serializeOperation,
  type Dependency,
} from "../operation-results.js";
import { modelProducer } from "./producer.js";
import {
  motivationCriterionSchema,
  type MotivationCriterionOutput,
} from "./schemas/motivation-criterion.js";

type Saved = {
  requestDigest: string;
  leaves: {
    key: string;
    question: string;
    materials: { key: string; reference: Dependency; text: string }[];
  }[];
  output: MotivationCriterionOutput;
  producerBytes: number[];
  evaluatedAt: string;
  modelInput: string;
};
/** Authenticated management operation. Ordinary MCP cannot start model evaluation.
 * Provider work uses one existing Subject binding; saved paid outputs resume owner commit. */
export async function evaluateMotivation(
  kernel: KernelClient,
  models: ModelRuntime,
  input: EvaluateConditionRequest,
  options: ExecutionOptions,
) {
  const calls = executionOptions(kernel.execution.opportunity, options);
  const identity = {
    subjectId: input.subjectId,
    owner: "motivation_evaluation",
    operationKey: input.operationId,
    semanticDigest: canonicalDigest({
      operation: "motivation.evaluation.v1",
      request: toJson(EvaluateConditionRequestSchema, input),
    }),
  };
  return serializeOperation(
    kernel,
    `motivation_evaluation:${input.subjectId}:${input.operationId}`,
    calls.signal,
    () =>
      runRecordedOperation<
        Saved,
        Awaited<ReturnType<KernelClient["motivation"]["evaluate"]>>
      >(kernel, identity, calls, {
        replay: (saved) => {
          if (saved.purged)
            throw new Error("Motivation evaluation source was purged");
          return fromJson(
            ConditionEvaluationSchema,
            JSON.parse(saved.payloadJson) as JsonValue,
          );
        },
        generate: async () => {
          const plan = await kernel.motivationWorkflow.prepare(input, calls);
          if (!plan.leaves.length)
            return { value: await kernel.motivation.evaluate(input, calls) };
          const jsonPlan = toJson(
            ConditionEvaluationPlanSchema,
            plan,
          ) as JsonObject;
          const modelInput = JSON.stringify({
            operation: "motivation_criterion",
            evaluationTime: jsonPlan.evaluated,
            criteria: jsonPlan.leaves,
          });
          const generated = await models.invocations.generate(
            "subject_reflection",
            { content: modelInput },
            {
              signal: calls.signal,
              operation: "motivation_criterion",
              prompt: {
                role: "subject_reflection",
                path: "motivation/criterion.md",
              },
              interpretOutput: (value) =>
                motivationCriterionSchema.parse(value),
            },
          );
          const output = generated.value;
          return {
            proposal: {
              modelInput,
              requestDigest: plan.requestDigest,
              leaves: plan.leaves.map((leaf) => ({
                key: leaf.key,
                question: leaf.question,
                materials: leaf.materials.map((m) => ({
                  key: m.key,
                  reference: {
                    kind: m.reference!.kind,
                    value: m.reference!.value,
                  },
                  text: m.text,
                })),
              })),
              output,
              producerBytes: Array.from(
                toBinary(
                  ProducerSignatureSchema,
                  modelProducer(
                    generated.producerMetadata,
                    "subject_reflection_text",
                  ),
                ),
              ),
              evaluatedAt: timestampDate(plan.evaluated!.at!).toISOString(),
            },
            dependencies: plan.dependencies,
            execution: generated.execution,
          };
        },
        commit: async () => {
          const result = await kernel.motivationWorkflow.apply(
            { request: input, identity },
            calls,
          );
          return {
            value: result,
            outcome: toJson(ConditionEvaluationSchema, result),
            dependencies: [input.conditionRevision!, ...input.evidenceRefs],
          };
        },
      }),
  );
}

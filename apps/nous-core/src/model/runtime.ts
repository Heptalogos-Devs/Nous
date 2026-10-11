// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { MaterialInterpretation } from "./interpretation.js";
import { createHash } from "node:crypto";
import { create } from "@bufbuild/protobuf";
import {
  ContextSegmentSchema,
  DegradationSchema,
  type Degradation,
  type ContextSegment as Segment,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { ModelInvocations } from "./invocations.js";
import {
  type ModelRoleSnapshot,
  type ModelProducerMetadata,
} from "./execution/snapshot.js";
import { type ExecutionTelemetry } from "./execution/routes.js";
import type { ModelGenerationOutput } from "./schemas/contracts.js";
import { type ModelRole } from "./roles.js";
import {
  modelConfigurationSchema,
  type ModelConfiguration,
} from "./configuration.js";

import { formationSchema } from "./schemas/formation.js";
import { projectionStewardSchema as proposalSchema } from "./schemas/projection.js";
type SemanticGeneration<R extends ModelRole> = {
  value: ModelGenerationOutput<R>;
  producerMetadata: ModelProducerMetadata;
  execution: ExecutionTelemetry;
};

export class ModelRuntime {
  readonly material: MaterialInterpretation;
  constructor(
    readonly invocations = new ModelInvocations(),
    readonly materialStrategy: ModelConfiguration["material_strategy"] = "description_only",
    readonly video = modelConfigurationSchema.parse({}).video,
    readonly credentialEnvironments: readonly string[] = [],
    readonly audio = modelConfigurationSchema.parse({}).audio,
    readonly tempRoot?: string,
    readonly materialInputs = modelConfigurationSchema.parse({})
      .material_inputs,
  ) {
    this.material = new MaterialInterpretation(invocations, video.prompt);
  }
  static async fromConfig(
    config: ModelConfiguration,
    promptRoot?: string,
    overridePromptRoot?: string,
    tempRoot?: string,
  ) {
    const invocations = await ModelInvocations.create(
      config,
      promptRoot,
      overridePromptRoot,
    );
    return new ModelRuntime(
      invocations,
      config.material_strategy,
      config.video,
      Object.values(config.gateway_profiles).map(
        (gateway) => gateway.credential_env,
      ),
      config.audio,
      tempRoot,
      config.material_inputs,
    );
  }
  get embeddingModel() {
    return this.invocations.profile("query_embedding")?.model;
  }
  async segmentEpisode(
    input: string,
    signal?: AbortSignal,
    snapshot?: ModelRoleSnapshot,
    beforeAttempt?: () => void,
  ): Promise<SemanticGeneration<"episode_segmentation">> {
    return this.invocations.generate(
      "episode_segmentation",
      { content: input },
      { signal: signal, snapshot: snapshot, beforeAttempt: beforeAttempt },
    );
  }
  async synthesizeJournal(
    input: string,
    signal?: AbortSignal,
    snapshot?: ModelRoleSnapshot,
    beforeAttempt?: () => void,
  ): Promise<SemanticGeneration<"journal_synthesis">> {
    return this.invocations.generate(
      "journal_synthesis",
      { content: input },
      { signal: signal, snapshot: snapshot, beforeAttempt: beforeAttempt },
    );
  }
  async maintainConcepts(
    input: string,
    signal?: AbortSignal,
    snapshot?: ModelRoleSnapshot,
    beforeAttempt?: () => void,
  ): Promise<SemanticGeneration<"concept_maintenance">> {
    return this.invocations.generate(
      "concept_maintenance",
      { content: input },
      { signal: signal, snapshot: snapshot, beforeAttempt: beforeAttempt },
    );
  }
  async consolidate(
    input: string,
    signal?: AbortSignal,
    snapshot?: ModelRoleSnapshot,
    beforeAttempt?: () => void,
  ): Promise<SemanticGeneration<"memory_consolidation">> {
    return this.invocations.generate(
      "memory_consolidation",
      { content: input },
      { signal: signal, snapshot: snapshot, beforeAttempt: beforeAttempt },
    );
  }
  async form(text: string, signal?: AbortSignal, snapshot?: ModelRoleSnapshot) {
    const result = await this.invocations.generate(
      "memory_formation",
      { content: text },
      { signal: signal, snapshot: snapshot },
    );
    return {
      ...formationSchema.parse(result.value),
      producerMetadata: result.producerMetadata,
      execution: result.execution,
    };
  }

  async refine(
    segments: Segment[],
    signal?: AbortSignal,
  ): Promise<{ segments: Segment[]; degradation: Degradation[] }> {
    if (!this.invocations.profile("projection_steward"))
      return {
        segments,
        degradation: [
          create(DegradationSchema, {
            code: "steward_not_configured",
            detail: "Deterministic projection",
          }),
        ],
      };
    try {
      const output = proposalSchema.parse(
        (
          await this.invocations.generate(
            "projection_steward",
            {
              content: JSON.stringify(
                segments.map((s) => ({
                  id: s.segmentId,
                  role: s.semanticRole,
                  text: s.text,
                })),
              ),
            },
            { signal: signal },
          )
        ).value,
      );
      const ids = new Set(segments.map((s) => s.segmentId));
      if (
        new Set(output.selectedIds).size !== output.selectedIds.length ||
        output.selectedIds.some((id) => !ids.has(id))
      )
        throw new Error("Steward proposed invalid source IDs");
      const selected = new Set(output.selectedIds);
      const sources = segments.filter((s) => selected.has(s.segmentId));
      if (output.summary) {
        if (!sources.length)
          throw new Error("A Steward summary requires selected sources");
        const revision = createHash("sha256")
          .update(
            JSON.stringify([
              this.invocations.identity("projection_steward"),
              sources,
              output.summary,
            ]),
          )
          .digest("hex");
        return {
          segments: [
            create(ContextSegmentSchema, {
              segmentId: `steward:${revision}`,
              text: output.summary,
              semanticRole: "steward_synthesis",
              sourceRefs: sources.flatMap((s) => s.sourceRefs),
              evidence: sources.flatMap((s) => s.evidence),
              authority: "interpretation",
              stability: "EPOCH_STABLE",
              sourceRevision: revision,
            }),
          ],
          degradation: [],
        };
      }
      return { segments: sources, degradation: [] };
    } catch (error) {
      if (signal?.aborted) throw signal.reason;
      return {
        segments,
        degradation: [
          create(DegradationSchema, {
            code: "steward_rejected",
            detail: error instanceof Error ? error.message : "Invalid proposal",
          }),
        ],
      };
    }
  }
}

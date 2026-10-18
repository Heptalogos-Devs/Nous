// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { Code, ConnectError } from "@connectrpc/connect";
import { create, toJson } from "@bufbuild/protobuf";
import {
  DegradationSchema,
  ContextSegmentSchema,
  ProjectionSchema,
  DomainStampSchema,
  DomainTemporalEvidenceSchema,
  type Projection,
  type ContextSegment as Segment,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import type { ConsumerPolicy } from "../domain.js";
import { ModelRuntime } from "../model/runtime.js";
import { canonicalDigest } from "../digest.js";
import { epistemicView } from "./epistemic.js";

function family(
  segment: Segment,
):
  | "memory"
  | "selfCognition"
  | "socialCognition"
  | "motivation"
  | "runtime"
  | "resource" {
  switch (segment.domain) {
    case "memory":
      return "memory";
    case "motivation":
      return "motivation";
    case "social":
      return "socialCognition";
    case "self":
      return "selfCognition";
    case "runtime":
      return "runtime";
    case "resource":
      return "resource";
    default:
      throw new ConnectError(
        "Contribution has no declared semantic owner",
        Code.Internal,
      );
  }
}
export class ProjectionPlanner {
  constructor(
    readonly policies: ReadonlyMap<string, ConsumerPolicy>,
    private readonly models = new ModelRuntime(),
  ) {}
  policy(id: string) {
    const policy = this.policies.get(id);
    if (!policy)
      throw new ConnectError("Unknown consumer policy", Code.PermissionDenied);
    return policy;
  }
  async build(
    batch: Projection,
    requested: { maxItems: number; maxTextBytes: number },
    policy: ConsumerPolicy,
    signal?: AbortSignal,
  ): Promise<Projection> {
    const items = Math.min(
      requested.maxItems || policy.maxItems,
      policy.maxItems,
    );
    const bytes = Math.min(
      requested.maxTextBytes || policy.maxTextBytes,
      policy.maxTextBytes,
    );
    if (items < 1 || bytes < 1)
      throw new ConnectError(
        "Projection budget must be positive",
        Code.InvalidArgument,
      );
    const required = batch.segments.filter(
      (s) => s.domain === "self" || policy[family(s)] === "REQUIRED",
    );
    const optional = batch.segments.filter(
      (s) => s.domain !== "self" && policy[family(s)] !== "REQUIRED",
    );
    const ownedCurrent = (source: Segment) =>
      source.semanticRole === "current_self" ||
      source.semanticRole === "current_social";
    const stewarded = optional.filter((source) => !ownedCurrent(source));
    let refined = stewarded;
    let synthesized = false;
    const degradation = [...batch.degradation];
    if (policy.materialize && stewarded.some((source) => source.text)) {
      try {
        const output = (
          await this.models.invocations.generate(
            "projection_steward",
            {
              content: JSON.stringify({
                semanticClock: batch.semanticClock
                  ? toJson(DomainStampSchema, batch.semanticClock)
                  : undefined,
                sources: stewarded.map((s) => ({
                  id: s.segmentId,
                  role: s.semanticRole,
                  text: s.text,
                  temporal: s.temporal
                    ? toJson(DomainTemporalEvidenceSchema, s.temporal)
                    : undefined,
                })),
              }),
            },
            { signal },
          )
        ).value;
        const ids = new Set(stewarded.map((s) => s.segmentId));
        if (
          new Set(output.selectedIds).size !== output.selectedIds.length ||
          output.selectedIds.some((id) => !ids.has(id))
        )
          throw new Error("Steward proposed invalid source IDs");
        const selected = new Set(output.selectedIds);
        refined = stewarded.filter((s) => selected.has(s.segmentId));
        if (output.summary) {
          if (!refined.length)
            throw new Error("A Steward summary requires selected sources");
          const revision = canonicalDigest([
            this.models.invocations.identity("projection_steward"),
            refined.map((source) => {
              const { segmentId: _id, ...content } = toJson(
                ContextSegmentSchema,
                source,
              ) as Record<string, unknown>;
              return content;
            }),
            output.summary,
          ]);
          synthesized = true;
          refined = [
            create(ContextSegmentSchema, {
              segmentId: `steward:${revision}`,
              text: output.summary,
              semanticRole: "steward_synthesis",
              sourceRefs: refined.flatMap((s) => s.sourceRefs),
              evidence: refined.flatMap((s) => s.evidence),
              authority: "interpretation",
              stability: "EPOCH_STABLE",
              sourceRevision: revision,
              temporal: {
                temporalEvidence: {
                  sources: [
                    ...new Map(
                      refined
                        .flatMap(
                          (s) => s.temporal?.temporalEvidence?.sources ?? [],
                        )
                        .map((s) => [s.occurrenceId, s]),
                    ).values(),
                  ],
                },
              },
              usage: "task_evidence",
              domain: "runtime",
            }),
          ];
        }
      } catch (error) {
        this.models.invocations.allowDegradation(
          error,
          signal,
          "projection_steward",
        );
        refined = stewarded;
        synthesized = false;
        degradation.push(
          create(DegradationSchema, {
            code: "steward_rejected",
            detail: error instanceof Error ? error.message : "Invalid proposal",
          }),
        );
      }
    }
    const refinedIds = new Set(refined.map((source) => source.segmentId));
    // Keep the owner's whole current units in their original selection order.
    const selectedOptional = synthesized
      ? optional.flatMap((source) =>
          ownedCurrent(source)
            ? [source]
            : source === stewarded[0]
              ? refined
              : [],
        )
      : optional.filter(
          (source) => ownedCurrent(source) || refinedIds.has(source.segmentId),
        );
    const selected = budget(
      [...required, ...selectedOptional],
      items,
      bytes,
      policy.materialize,
    );
    const selectedIds = new Set(selected.map((segment) => segment.segmentId));
    if (
      required.some(
        (segment) => segment.required && !selectedIds.has(segment.segmentId),
      )
    )
      throw new ConnectError(
        "Required cognition does not fit the input budget",
        Code.ResourceExhausted,
      );
    if (selected.length < required.length + selectedOptional.length)
      degradation.push(
        create(DegradationSchema, {
          code: "context_budget_omission",
          detail: "Optional semantic units omitted whole",
        }),
      );
    const segments = selected.map((segment) => {
      // Contribution IDs identify one source read; projected IDs identify the
      // actual selected content, evidence and revision delivered to a consumer.
      const { segmentId: _contributionId, ...content } = segment;
      return {
        ...content,
        segmentId: canonicalDigest([
          1,
          toJson(
            ContextSegmentSchema,
            create(ContextSegmentSchema, { ...content, segmentId: "" }),
          ),
        ]),
      };
    });
    // Required owners must be available, but an empty relevant view is valid:
    // a new Subject has no adopted cognition to invent. Kernel checks owner
    // availability; mandatory identity/guidance units above must still fit.
    const projectionId = canonicalDigest([
      policy.revision,
      segments.map((segment) =>
        toJson(ContextSegmentSchema, create(ContextSegmentSchema, segment)),
      ),
    ]);
    return create(ProjectionSchema, {
      ...batch,
      projectionId,
      epistemicView: epistemicView(segments, degradation),
      segments,
      degradation,
    });
  }
}
function budget(
  segments: Segment[],
  items: number,
  maxBytes: number,
  materialize: boolean,
): Segment[] {
  const result: Segment[] = [];
  let bytes = 0;
  for (const source of segments) {
    if (result.length >= items) break;
    const text = materialize ? source.text : "";
    if (Buffer.byteLength(text) > maxBytes - bytes) continue;
    if (source.text && materialize && !text) continue;
    bytes += Buffer.byteLength(text);
    result.push({ ...source, text });
  }
  return result;
}

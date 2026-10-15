// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { describe, expect, it, vi } from "vitest";
import { ContextCompiler } from "../src/cognition/context.js";
import { ModelRuntime } from "../src/model/runtime.js";
import { ProjectionPlanner } from "../src/cognition/projection.js";
import { CoreCognition } from "../src/cognition/service.js";
import { coreExecutionSchema } from "../src/configuration/catalog.js";
import { executionOptions } from "../src/execution.js";
import type { KernelClient } from "../src/kernel-client.js";
import { ResourceRegistry } from "../src/resources/registry.js";
import { create, fromBinary, toBinary } from "@bufbuild/protobuf";
import {
  CognitiveRefSchema,
  ContextSegmentSchema,
  EvidenceSchema,
  ProjectionRequestSchema,
  ProjectionSchema,
  type ContextSegment as Segment,
  type Projection,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import type { ConsumerPolicy } from "../src/domain.js";

const segment = (id: string, text = id): Segment =>
  create(ContextSegmentSchema, {
    segmentId: id,
    text,
    semanticRole: "evidence",
    sourceRefs: [{ kind: "memory", value: id }],
    evidence: [],
    authority: "subject_cognition",
    stability: "EPOCH_STABLE",
  });
const projection = (...segments: Segment[]): Projection =>
  create(ProjectionSchema, {
    projectionId: "p",
    consumerId: "test",
    sourceRuntimeRevision: 1n,
    segments,
    degradation: [],
  });
const policy: ConsumerPolicy = {
  consumerId: "test",
  revision: "1",
  memory: "PREFERRED",
  resource: "FORBIDDEN",
  runtime: "OPTIONAL",
  maxItems: 10,
  maxTextBytes: 20,
  materialize: true,
};

describe("managed context synchronization", () => {
  it("appends only to a synchronized immutable prefix and resets on source revision", () => {
    const compiler = new ContextCompiler(256);
    const key = { subjectId: "s", sessionId: "session", consumerId: "test" };
    const initial = compiler.compile(key, "1", projection(segment("a")));
    expect(initial.kind).toBe("RESET");
    const append = compiler.compile(
      key,
      "1",
      projection(segment("a"), segment("b")),
      initial.cursor,
    );
    expect(append.kind).toBe("APPEND");
    expect(append.projection.segments.map((s) => s.segmentId)).toEqual(["b"]);
    expect(
      compiler.compile(
        key,
        "1",
        projection(segment("a", "revised"), segment("b")),
        append.cursor,
      ).kind,
    ).toBe("RESET");
    expect(
      compiler.compile(
        { ...key, consumerId: "another" },
        "1",
        projection(segment("a")),
        initial.cursor,
      ).kind,
    ).toBe("RESET");
    expect(
      new ContextCompiler(256).compile(
        key,
        "1",
        projection(segment("a")),
        initial.cursor,
      ).kind,
    ).toBe("RESET");
  });
});
describe("projection authority boundaries", () => {
  it("keeps projected segment identity stable across fresh contribution IDs and resets on evidence drift", async () => {
    let batch = projection();
    const execution = coreExecutionSchema.parse(undefined);
    const kernel = {
      execution,
      runtime: { getSession: async () => ({ runtimeRevision: 1n }) },
      projection: { buildContributionBatch: async () => batch },
    } as unknown as KernelClient;
    const service = new CoreCognition(
      kernel,
      new ModelRuntime(),
      new ResourceRegistry(),
      [policy],
    );
    const build = async (input: Segment) => {
      batch = projection(input);
      const result = await service.buildProjection(
        create(ProjectionRequestSchema, {
          subjectId: "s",
          sessionId: "session",
          consumerId: "test",
          maxItems: 5,
          maxTextBytes: 20,
        }),
        executionOptions(execution.opportunity),
      );
      return fromBinary(ProjectionSchema, toBinary(ProjectionSchema, result));
    };
    const compiler = new ContextCompiler(256);
    const key = { subjectId: "s", sessionId: "session", consumerId: "test" };
    const original = create(ContextSegmentSchema, {
      ...segment("source", "same content"),
      evidence: [
        create(EvidenceSchema, {
          basisRole: "direct",
          reference: create(CognitiveRefSchema, {
            kind: "occurrence",
            value: "source",
          }),
          epistemicRelation: "supports",
        }),
      ],
    });
    const first = await build(original);
    expect(first.segments[0]?.evidence[0]?.epistemicRelation).toBe("supports");
    const second = await build({
      ...original,
      segmentId: "fresh contribution ID",
    });
    expect(second.segments[0]?.segmentId).toBe(first.segments[0]?.segmentId);
    expect(second.projectionId).toBe(first.projectionId);
    const initial = compiler.compile(key, "1", first);
    const unchanged = compiler.compile(key, "1", second, initial.cursor);
    expect(unchanged.kind).toBe("APPEND");
    expect(unchanged.projection.segments).toEqual([]);
    expect(unchanged.cursor).toEqual(initial.cursor);
    const changed = await build(
      create(ContextSegmentSchema, {
        ...original,
        evidence: [
          create(EvidenceSchema, {
            basisRole: "direct",
            reference: create(CognitiveRefSchema, {
              kind: "occurrence",
              value: "source",
            }),
            epistemicRelation: "contradicts",
          }),
        ],
      }),
    );
    expect(changed.segments[0]?.evidence[0]?.epistemicRelation).toBe(
      "contradicts",
    );
    expect(compiler.compile(key, "1", changed, unchanged.cursor).kind).toBe(
      "RESET",
    );
  });
  it("rejects invented model refs and preserves deterministic source selection", async () => {
    const model = new ModelRuntime();
    vi.spyOn(model.invocations, "profile").mockReturnValue({} as never);
    const generate = vi.spyOn(model.invocations, "generate").mockResolvedValue({
      value: { selectedIds: ["invented"] },
    } as never);
    const planner = new ProjectionPlanner(new Map([["test", policy]]), model);
    const refsOnly = await planner.build(
      projection(segment("a")),
      { maxItems: 5, maxTextBytes: 20 },
      { ...policy, materialize: false },
    );
    expect(refsOnly.segments[0]?.text).toBe("");
    expect(generate).not.toHaveBeenCalled();
    const result = await planner.build(
      projection(segment("a")),
      { maxItems: 5, maxTextBytes: 20 },
      policy,
    );
    expect(result.segments.map((s) => s.sourceRefs)).toEqual([
      [create(CognitiveRefSchema, { kind: "memory", value: "a" })],
    ]);
    expect(result.degradation[0]?.code).toBe("steward_rejected");
  });
  it("omits whole units and refuses to remove mandatory identity", async () => {
    const required = { ...policy, memory: "REQUIRED" as const };
    const planner = new ProjectionPlanner(
      new Map([["test", required]]),
      new ModelRuntime(),
    );
    const empty = await planner.build(
      projection(),
      { maxItems: 1, maxTextBytes: 5 },
      required,
    );
    expect(empty.segments).toHaveLength(0);
    expect(empty.degradation).toEqual([]);
    const result = await planner.build(
      projection(segment("a", "中文中文")),
      {
        maxItems: 1,
        maxTextBytes: 5,
      },
      required,
    );
    expect(result.segments).toHaveLength(0);
    expect(result.degradation.map((value) => value.code)).toContain(
      "context_budget_omission",
    );
    const replacement = await planner.build(
      projection(segment("a", "�X")),
      { maxItems: 1, maxTextBytes: 3 },
      required,
    );
    expect(replacement.segments).toHaveLength(0);
    await expect(
      planner.build(
        projection({
          ...segment("self", "Only under the full stated condition."),
          usage: "identity",
        }),
        { maxItems: 1, maxTextBytes: 3 },
        required,
      ),
    ).rejects.toMatchObject({ code: 8 });
  });
});

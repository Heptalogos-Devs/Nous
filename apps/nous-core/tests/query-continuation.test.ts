// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { create } from "@bufbuild/protobuf";
import { expect, it, vi } from "vitest";
import { coreExecutionSchema } from "../src/configuration/catalog.js";
import type { KernelClient } from "../src/kernel-client.js";
import type { ModelRuntime } from "../src/model/runtime.js";
import { QueryOrchestrator } from "../src/query/orchestrator.js";
import { ResourceRegistry } from "../src/resources/registry.js";
import type { ExternalResourceAdapter } from "../src/resources/adapter.js";
import {
  EmbeddingNeedsResponseSchema,
  QueryEmbeddingWorkSchema,
  QueryHostRequirement,
  QueryRunUpdateSchema,
  type QueryRunRequest,
  type QueryRunUpdate,
} from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/model_pb.js";
import {
  CognitiveRefSchema,
  QueryExprSchema,
  QueryRequestSchema,
  QueryResponseSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";

const queryRequest = () =>
  create(QueryRequestSchema, {
    subjectId: "subject",
    expression: create(QueryExprSchema, {
      operation: "atom",
      cues: [{ cue: { case: "text", value: "release status" } }],
    }),
  });

const embeddingConfig = {
  spaceHash: "space",
  producerHash: "producer",
  producerHashes: ["producer"],
  model: "fake",
  dimension: 2,
};

const embeddingUpdate = (historicalView = false): QueryRunUpdate =>
  create(QueryRunUpdateSchema, {
    frame: {
      case: "embedding",
      value: create(QueryEmbeddingWorkSchema, {
        representation: "Intent: release status",
        config: embeddingConfig,
        historicalView,
      }),
    },
  });

const completeUpdate = (): QueryRunUpdate =>
  create(QueryRunUpdateSchema, {
    frame: {
      case: "complete",
      value: create(QueryResponseSchema, { status: "complete" }),
    },
  });

const workflow = (
  requests: AsyncIterable<QueryRunRequest>,
  run: (
    frames: AsyncIterator<QueryRunRequest, undefined>,
  ) => AsyncGenerator<QueryRunUpdate>,
) => ({
  async *[Symbol.asyncIterator]() {
    const input = requests[Symbol.asyncIterator]();
    let pending = input.next();
    const frames: AsyncIterator<QueryRunRequest, undefined> = {
      async next() {
        const frame = await pending;
        pending = input.next();
        return frame;
      },
    };
    try {
      yield* run(frames);
    } finally {
      await input.return?.();
    }
  },
});

it("runs host embedding and returns completion on one bidi stream", async () => {
  const runQuery = vi.fn((requests: AsyncIterable<QueryRunRequest>) =>
    workflow(requests, async function* (frames) {
      expect((await frames.next()).value?.frame.case).toBe("start");
      yield embeddingUpdate();
      const outcome = (await frames.next()).value;
      expect(outcome?.frame.case).toBe("embedding");
      yield completeUpdate();
    }),
  );
  const embeddingBatch = vi.fn(async (texts: string[]) => ({
    value: texts.map(() => [1, 0]),
    producer: { signature_hash: "producer" },
  }));
  const kernel = {
    execution: coreExecutionSchema.parse(undefined),
    queryWorkflow: { runQuery },
  } as unknown as KernelClient;
  const models = {
    embeddingModel: {},
    invocations: {
      embeddingBatch,
      profile: () => undefined,
      requirement: () => "optional",
    },
  } as unknown as ModelRuntime;

  const result = await new QueryOrchestrator(
    kernel,
    models,
    new ResourceRegistry(),
  ).execute(queryRequest());

  expect(embeddingBatch).toHaveBeenCalledOnce();
  expect(embeddingBatch).toHaveBeenCalledWith(
    ["Intent: release status"],
    "fake",
    expect.any(AbortSignal),
  );
  expect(runQuery).toHaveBeenCalledOnce();
  expect(runQuery).toHaveBeenCalledWith(
    expect.anything(),
    expect.objectContaining({ timeoutMs: 300000 }),
  );
  expect(result.status).toBe("complete");
});

it("uses the same stream for historical embedding discovery and commit", async () => {
  const frames: string[] = [];
  const runQuery = vi.fn((requests: AsyncIterable<QueryRunRequest>) =>
    workflow(requests, async function* (inbound) {
      expect((await inbound.next()).value?.frame.case).toBe("start");
      yield embeddingUpdate(true);
      const needs = (await inbound.next()).value;
      frames.push(needs?.frame.case ?? "missing");
      expect(needs?.frame.case).toBe("embeddingNeeds");
      yield create(QueryRunUpdateSchema, {
        frame: {
          case: "embeddingNeeds",
          value: create(EmbeddingNeedsResponseSchema, {
            config: embeddingConfig,
            needs: [
              {
                references: [
                  create(CognitiveRefSchema, {
                    kind: "occurrence",
                    value: "source",
                  }),
                ],
                text: "historical material",
                digest: "digest",
              },
            ],
          }),
        },
      });
      const commit = (await inbound.next()).value;
      frames.push(commit?.frame.case ?? "missing");
      expect(commit?.frame.case).toBe("commitEmbedding");
      yield create(QueryRunUpdateSchema, {
        frame: { case: "embeddingCommitted", value: {} },
      });
      const embedding = (await inbound.next()).value;
      frames.push(embedding?.frame.case ?? "missing");
      expect(embedding?.frame.case).toBe("embedding");
      yield completeUpdate();
    }),
  );
  const embeddingBatch = vi.fn(async (texts: string[]) => ({
    value: texts.map(() => [1, 0]),
    producer: { signature_hash: "producer" },
  }));
  const kernel = {
    execution: coreExecutionSchema.parse(undefined),
    queryWorkflow: { runQuery },
  } as unknown as KernelClient;
  const models = {
    embeddingModel: {},
    invocations: {
      embeddingBatch,
      profile: () => undefined,
      requirement: () => "optional",
    },
  } as unknown as ModelRuntime;

  await new QueryOrchestrator(kernel, models, new ResourceRegistry()).execute(
    queryRequest(),
  );

  expect(frames).toEqual(["embeddingNeeds", "commitEmbedding", "embedding"]);
  expect(embeddingBatch).toHaveBeenCalledTimes(2);
  expect(runQuery).toHaveBeenCalledOnce();
});

it("fences query-vector caching across subject invalidation", async () => {
  let releaseEmbedding!: (value: {
    value: number[][];
    producer: { signature_hash: string };
  }) => void;
  const delayedEmbedding = new Promise<{
    value: number[][];
    producer: { signature_hash: string };
  }>((resolve) => {
    releaseEmbedding = resolve;
  });
  const embeddingBatch = vi
    .fn()
    .mockReturnValueOnce(delayedEmbedding)
    .mockImplementation(async (texts: string[]) => ({
      value: texts.map(() => [1, 0]),
      producer: { signature_hash: "producer" },
    }));
  const runQuery = vi.fn((requests: AsyncIterable<QueryRunRequest>) =>
    workflow(requests, async function* (frames) {
      expect((await frames.next()).value?.frame.case).toBe("start");
      yield embeddingUpdate();
      expect((await frames.next()).value?.frame.case).toBe("embedding");
      yield completeUpdate();
    }),
  );
  const orchestrator = new QueryOrchestrator(
    {
      execution: coreExecutionSchema.parse(undefined),
      queryWorkflow: { runQuery },
    } as unknown as KernelClient,
    {
      embeddingModel: {},
      invocations: {
        embeddingBatch,
        profile: () => undefined,
        requirement: () => "optional",
      },
    } as unknown as ModelRuntime,
    new ResourceRegistry(),
  );

  const firstRun = orchestrator.execute(queryRequest());
  await vi.waitFor(() => expect(embeddingBatch).toHaveBeenCalledOnce());
  orchestrator.invalidateSubject("subject");
  releaseEmbedding({
    value: [[1, 0]],
    producer: { signature_hash: "producer" },
  });
  await firstRun;
  await orchestrator.execute(queryRequest());
  expect(embeddingBatch).toHaveBeenCalledTimes(2);
});

it("does not execute resources when required reranking is unavailable", async () => {
  let rerankProfileAvailable = true;
  const search = vi.fn(async () => []);
  const runQuery = vi.fn((requests: AsyncIterable<QueryRunRequest>) =>
    workflow(requests, async function* (frames) {
      expect((await frames.next()).value?.frame.case).toBe("start");
      yield create(QueryRunUpdateSchema, {
        frame: {
          case: "finalize",
          value: {
            baseline: create(QueryResponseSchema, {
              status: "complete",
              hits: [{ text: "one" }, { text: "two" }],
              resourceActions: [
                {
                  resourceRef: "resource:docs",
                  action: "search",
                  actionId: "action",
                  queryText: "release",
                  limit: 1,
                  adapterKind: "test",
                  providerProfile: "test",
                },
              ],
            }),
            rerankQuery: "release status",
            rerankCandidateLimit: 2,
            rerankExpected: true,
            rerankRequirement: QueryHostRequirement.REQUIRED,
          },
        },
      });
      rerankProfileAvailable = false;
      const finalize = (await frames.next()).value;
      expect(finalize?.frame.case).toBe("finalize");
      if (finalize?.frame.case === "finalize") {
        expect(finalize.frame.value.resources).toEqual([]);
        expect(finalize.frame.value.rerank?.outcome?.case).toBe("notRequested");
      }
      yield completeUpdate();
    }),
  );
  const kernel = {
    execution: coreExecutionSchema.parse(undefined),
    queryWorkflow: { runQuery },
  } as unknown as KernelClient;
  const models = {
    invocations: {
      profile: (role: string) =>
        role === "query_rerank" && rerankProfileAvailable
          ? { model: "fake" }
          : undefined,
      requirement: (role: string) =>
        role === "query_rerank" ? "required" : "optional",
    },
  } as unknown as ModelRuntime;
  const resources = new ResourceRegistry([
    {
      adapterKind: "test",
      providerProfile: "test",
      adapter: {
        describe: vi.fn(async () => ({ resourceIds: [], accessible: true })),
        search,
        materialize: vi.fn(async () => ({})),
      } as unknown as ExternalResourceAdapter,
    },
  ]);

  await new QueryOrchestrator(kernel, models, resources).execute(
    queryRequest(),
  );

  expect(search).not.toHaveBeenCalled();
});

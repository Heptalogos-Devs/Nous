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
  QueryHostRequirement,
  type QueryRunRequest,
} from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/model_pb.js";
import {
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

it("continues one QueryRun through Core embedding and returns its completion", async () => {
  const runQuery = vi.fn(async (request: QueryRunRequest) => {
    if (request.command.case === "start") {
      return {
        handle: { queryRunId: "query-run", revision: 1n },
        next: {
          case: "embedding" as const,
          value: {
            representation: "Intent: release status",
            config: {
              spaceHash: "space",
              producerHash: "producer",
              producerHashes: ["producer"],
              model: "fake",
              dimension: 2,
            },
            historicalView: false,
          },
        },
      };
    }
    expect(request.command.case).toBe("resume");
    if (request.command.case !== "resume") throw new Error("resume expected");
    expect(request.command.value.handle).toMatchObject({
      queryRunId: "query-run",
      revision: 1n,
    });
    expect(request.command.value.outcome?.case).toBe("embedding");
    return {
      handle: undefined,
      next: {
        case: "complete" as const,
        value: create(QueryResponseSchema, { status: "complete" }),
      },
    };
  });
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
  expect(runQuery).toHaveBeenCalledTimes(2);
  expect(runQuery.mock.calls[0]?.[0].command.case).toBe("start");
  expect(runQuery.mock.calls[1]?.[0].command.case).toBe("resume");
  expect(result.status).toBe("complete");
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
  const runQuery = vi.fn(async (request: QueryRunRequest) => {
    if (request.command.case === "start") {
      return {
        handle: {
          queryRunId: `run-${runQuery.mock.calls.length}`,
          revision: 1n,
        },
        next: {
          case: "embedding" as const,
          value: {
            representation: "Intent: release status",
            config: {
              spaceHash: "space",
              producerHash: "producer",
              producerHashes: ["producer"],
              model: "fake",
              dimension: 2,
            },
            historicalView: false,
          },
        },
      };
    }
    return {
      handle: undefined,
      next: {
        case: "complete" as const,
        value: create(QueryResponseSchema, { status: "complete" }),
      },
    };
  });
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

it("does not run resource adapters when required reranking was not requested", async () => {
  let rerankProfileAvailable = true;
  const search = vi.fn(async () => []);
  const runQuery = vi.fn(async (request: QueryRunRequest) => {
    if (request.command.case === "start") {
      rerankProfileAvailable = false;
      return {
        handle: { queryRunId: "query-run", revision: 1n },
        next: {
          case: "finalize" as const,
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
      };
    }
    if (request.command.case === "resume") {
      expect(request.command.value.outcome?.case).toBe("finalize");
      if (request.command.value.outcome?.case === "finalize") {
        expect(request.command.value.outcome.value.resources).toEqual([]);
        expect(request.command.value.outcome.value.rerank?.outcome?.case).toBe(
          "notRequested",
        );
      }
      return {
        handle: undefined,
        next: {
          case: "complete" as const,
          value: create(QueryResponseSchema, { status: "complete" }),
        },
      };
    }
    return { handle: undefined, next: undefined, cancelled: true };
  });
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

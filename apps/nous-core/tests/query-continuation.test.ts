// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { coreExecutionSchema } from "../src/configuration/catalog.js";
import { expect, it, vi } from "vitest";
import { create } from "@bufbuild/protobuf";
import {
  QueryExprSchema,
  QueryRequestSchema,
  QueryResponseSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { QueryOrchestrator } from "../src/query/orchestrator.js";
import type { KernelClient } from "../src/kernel-client.js";
import type { ModelRuntime } from "../src/model/runtime.js";
import type { ResourceRegistry } from "../src/resources/registry.js";

it("finishes a Resource ticket without invoking required rerank for identity-only intent", async () => {
  const response = create(QueryResponseSchema, {
    status: "complete",
    hits: [{ text: "first" }, { text: "second" }],
  });
  const finalize = vi.fn(async () => response);
  const release = vi.fn(async () => ({}));
  const rerank = vi.fn(async () => {
    throw new Error("Empty intent must not enter the reranker");
  });
  const kernel = {
    execution: coreExecutionSchema.parse(undefined),
    queryWorkflow: {
      prepareQuery: async () => ({
        preparationToken: "prepared-token",
        boundQuery: "{}",
        embeddingRequired: false,
        embeddingText: "",
      }),
      query: async () => ({ response, validationTicket: "resource-ticket" }),
      finalizeQuery: finalize,
      releaseQuery: release,
    },
  } as unknown as KernelClient;
  const models = {
    invocations: {
      profile: () => ({ protocol: "rerank-v1", model: "required-model" }),
      requirement: () => "required",
      rerank,
    },
  } as unknown as ModelRuntime;
  await new QueryOrchestrator(kernel, models, {} as ResourceRegistry).execute(
    create(QueryRequestSchema, {
      subjectId: "subject",
      expression: create(QueryExprSchema, { operation: "atom" }),
    }),
  );
  expect(rerank).not.toHaveBeenCalled();
  expect(finalize).toHaveBeenCalledOnce();
  expect(release).toHaveBeenCalledTimes(2);
});

it("prepares before embedding and sends one complete representation for an expression tree", async () => {
  const representation =
    "Intent:\nALL OF: Alice's release; Nous Wave tasks\n\nCurrent work:\nRelease stabilization";
  const embedding = vi.fn(async (texts: string[]) => ({
    value: texts.map(() => [1, 0]),
    producer: { signature_hash: "producer" },
  }));
  const execute = vi.fn(async () => ({
    response: create(QueryResponseSchema, {
      status: "complete",
      boundQuery: '{"activation":true}',
    }),
  }));
  const release = vi.fn(async () => ({}));
  const prepare = vi.fn(async () => ({
    preparationToken: "prepared",
    boundQuery: '{"prepared":true}',
    embeddingText: representation,
    embeddingRequired: true,
    rerankRequirement: "forbidden",
  }));
  const kernel = {
    execution: coreExecutionSchema.parse(undefined),
    materialWorkflow: {
      getEmbeddingConfig: async () => ({
        spaceHash: "space",
        producerHash: "producer",
        producerHashes: ["producer"],
        model: "fake",
      }),
    },
    queryWorkflow: {
      prepareQuery: prepare,
      query: execute,
      releaseQuery: release,
    },
  } as unknown as KernelClient;
  const models = {
    embeddingModel: {},
    invocations: {
      embeddingBatch: embedding,
      profile: () => ({ model: "configured-reranker" }),
      requirement: (role: string) =>
        role === "query_rerank" ? "required" : "optional",
      rerank: vi.fn(async () => {
        throw new Error("forbidden rerank invoked");
      }),
    },
  } as unknown as ModelRuntime;
  const expression = create(QueryExprSchema, {
    operation: "all",
    children: [
      {
        operation: "atom",
        cues: [{ cue: { case: "text", value: "Alice's release" } }],
      },
      {
        operation: "atom",
        cues: [{ cue: { case: "text", value: "Nous Wave tasks" } }],
      },
    ],
  });
  const result = await new QueryOrchestrator(
    kernel,
    models,
    {} as ResourceRegistry,
  ).execute(
    create(QueryRequestSchema, {
      subjectId: "subject",
      expression,
      capabilities: { rerank: "forbidden" },
    }),
  );
  expect(prepare).toHaveBeenCalledOnce();
  expect(embedding).toHaveBeenCalledOnce();
  expect(embedding.mock.calls[0]?.[0]).toEqual([representation]);
  expect(execute).toHaveBeenCalledWith(
    expect.objectContaining({
      subjectId: "subject",
      preparationToken: "prepared",
      embeddings: [expect.objectContaining({ text: representation })],
    }),
    expect.anything(),
  );
  expect(result.boundQuery).toBe('{"activation":true}');
  expect(release).toHaveBeenCalledOnce();
});

it.each([
  ["off", "optional", false],
  ["model", "forbidden", false],
  ["model", "optional", true],
  ["model", "required", true],
])(
  "Host query concept model obeys mode=%s and requirement=%s",
  async (mode, requirement, allowed) => {
    const events: string[] = [];
    const activation = vi.fn(async () => {
      events.push("activation");
      return {
        preparationToken: "activated",
        modelInput: '{"existing_tags":[{"key":"c0"}]}',
      };
    });
    const generate = vi.fn(async () => {
      events.push("model");
      return {
        value: {
          existing_tags: [{ key: "c0", strength: 0.8 }],
          novel_concepts: [],
        },
      };
    });
    const execute = vi.fn(async () => {
      events.push("query");
      return {
        response: create(QueryResponseSchema, {
          status: "complete",
          boundQuery: '{"model_calls":1}',
        }),
      };
    });
    const release = vi.fn(async () => ({}));
    const kernel = {
      execution: coreExecutionSchema.parse(undefined),
      queryWorkflow: {
        prepareQuery: async () => ({
          preparationToken: "prepared",
          embeddingRequired: false,
          embeddingText: "Intent:\nreader leases",
          rerankRequirement: "forbidden",
          conceptEnrichmentMode: mode,
          conceptEnrichmentRequirement: requirement,
        }),
        activateQuery: activation,
        query: execute,
        releaseQuery: release,
      },
    } as unknown as KernelClient;
    const models = {
      invocations: {
        profile: () => ({ model: "fake" }),
        requirement: () => "optional",
        generate,
      },
    } as unknown as ModelRuntime;
    await new QueryOrchestrator(kernel, models, {} as ResourceRegistry).execute(
      create(QueryRequestSchema, { subjectId: "subject" }),
    );
    if (allowed) {
      expect(events).toEqual(["activation", "model", "query"]);
      expect(execute).toHaveBeenCalledWith(
        expect.objectContaining({
          preparationToken: "activated",
          embeddings: [],
          conceptOutput: JSON.stringify({
            existing_tags: [{ key: "c0", strength: 0.8 }],
            novel_concepts: [],
          }),
        }),
        expect.anything(),
      );
      expect(release).toHaveBeenCalledWith(
        { subjectId: "subject", validationTicket: "activated" },
        expect.anything(),
      );
    } else {
      expect(events).toEqual(["query"]);
      expect(generate).not.toHaveBeenCalled();
    }
  },
);

it.each(["optional", "required"])(
  "missing Host concept role obeys %s and releases its activation lease",
  async (requirement) => {
    const execute = vi.fn(
      async (_request: {
        conceptFailure?: string;
        conceptModelCalls?: number;
      }) => ({
        response: create(QueryResponseSchema, { status: "degraded" }),
      }),
    );
    const release = vi.fn(async () => ({}));
    const kernel = {
      execution: coreExecutionSchema.parse(undefined),
      queryWorkflow: {
        prepareQuery: async () => ({
          preparationToken: "prepared",
          embeddingRequired: false,
          conceptEnrichmentMode: "model",
          conceptEnrichmentRequirement: requirement,
        }),
        activateQuery: async () => ({
          preparationToken: "activated",
          modelInput: "{}",
        }),
        query: execute,
        releaseQuery: release,
      },
    } as unknown as KernelClient;
    const models = {
      invocations: { profile: () => undefined, requirement: () => "optional" },
    } as unknown as ModelRuntime;
    const result = new QueryOrchestrator(
      kernel,
      models,
      {} as ResourceRegistry,
    ).execute(create(QueryRequestSchema, { subjectId: "subject" }));
    if (requirement === "required") {
      await expect(result).rejects.toThrow(
        "Required query concept model unavailable",
      );
      expect(execute).not.toHaveBeenCalled();
    } else {
      await result;
      expect(execute.mock.calls[0]?.[0].conceptFailure).toContain(
        "Query concept model role unavailable",
      );
      expect(execute.mock.calls[0]?.[0].conceptModelCalls).toBe(0);
    }
    expect(release).toHaveBeenCalledWith(
      { subjectId: "subject", validationTicket: "activated" },
      expect.anything(),
    );
  },
);

it("discovers and commits historical document embeddings against the reserved query view", async () => {
  const listEmbeddingNeeds = vi.fn(async () => ({
    config: {
      spaceHash: "space",
      producerHash: "producer",
      producerHashes: ["producer"],
      model: "fake",
    },
    needs: [
      {
        reference: { kind: "tag", value: "old" },
        text: "Concept: old meaning",
        digest: "old",
      },
    ],
  }));
  const commitEmbedding = vi.fn(async () => ({}));
  const embeddingBatch = vi.fn(async (texts: string[]) => ({
    value: texts.map(() => [1, 0]),
    producer: { signature_hash: "producer" },
  }));
  const kernel = {
    execution: coreExecutionSchema.parse(undefined),
    materialWorkflow: {
      listEmbeddingNeeds,
      commitEmbedding,
      getEmbeddingConfig: async () => ({
        spaceHash: "space",
        producerHash: "producer",
        producerHashes: ["producer"],
        model: "fake",
      }),
    },
    queryWorkflow: {
      prepareQuery: async () => ({
        preparationToken: "frozen-history",
        historicalView: true,
        embeddingRequired: true,
        embeddingText: "current question with old concepts",
        rerankRequirement: "forbidden",
      }),
      query: async () => ({
        response: create(QueryResponseSchema, { status: "complete" }),
      }),
      releaseQuery: async () => ({}),
    },
  } as unknown as KernelClient;
  const models = {
    embeddingModel: {},
    invocations: {
      embeddingBatch,
      profile: () => ({ embedding: { max_batch_size: 64 } }),
      requirement: () => "optional",
    },
  } as unknown as ModelRuntime;
  await new QueryOrchestrator(kernel, models, {} as ResourceRegistry).execute(
    create(QueryRequestSchema, { subjectId: "subject" }),
  );
  expect(listEmbeddingNeeds).toHaveBeenCalledWith(
    { subjectId: "subject", limit: 256, preparationToken: "frozen-history" },
    expect.anything(),
  );
  expect(commitEmbedding).toHaveBeenCalledWith(
    expect.objectContaining({
      subjectId: "subject",
      preparationToken: "frozen-history",
      material: {
        text: "Concept: old meaning",
        spaceHash: "space",
        producerHash: "producer",
        vector: [1, 0],
      },
    }),
    expect.anything(),
  );
  expect(embeddingBatch.mock.calls.map(([texts]) => texts)).toEqual([
    ["Concept: old meaning"],
    ["current question with old concepts"],
  ]);
});

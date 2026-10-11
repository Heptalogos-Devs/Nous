// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { afterEach, describe, expect, it } from "vitest";
import { mkdtemp, mkdir, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createServer } from "node:http";
import { modelConfigurationSchema } from "../../src/model/configuration.js";
import { ModelInvocations } from "../../src/model/invocations.js";
import { failedExecutionTelemetry } from "../../src/model/execution/routes.js";
import { PromptRegistry } from "../../src/model/prompts.js";
import { providerContractForRole } from "../../src/model/schemas/contracts.js";

const temporary: string[] = [];
afterEach(async () => {
  await Promise.all(
    temporary.splice(0).map((p) => rm(p, { recursive: true, force: true })),
  );
});
describe("model protocol and provenance boundaries", () => {
  it("rejects unsafe destinations and structurally misbound roles", async () => {
    for (const base_url of [
      "http://gateway.example/v1",
      "https://user:secret@gateway.example/v1",
      "https://gateway.example/v1?token=secret",
      "http://localhost/v1",
    ])
      expect(
        modelConfigurationSchema.safeParse({
          gateway_profiles: { primary: { base_url, credential_env: "TOKEN" } },
        }).success,
      ).toBe(false);
    expect(
      modelConfigurationSchema.parse({
        gateway_profiles: {
          primary: {
            base_url: "http://127.0.0.1:9000/v1/",
            credential_env: "TOKEN",
          },
        },
      }).gateway_profiles.primary?.base_url,
    ).toBe("http://127.0.0.1:9000/v1");
    const configuration = modelConfigurationSchema.safeParse({
      gateway_profiles: {
        local: {
          base_url: "https://example.com/v1",
          credential_env: "TOKEN",
        },
      },
      model_profiles: {
        chat: {
          gateway: "local",
          protocol: "openai-chat",
          model: "chat",
          capabilities: ["text"],
        },
      },
      execution_profiles: { chat: { model: "chat" } },
      roles: { query_embedding: { routes: ["chat"] } },
    });
    expect(configuration.success).toBe(false);
    if (!configuration.success)
      expect(configuration.error.issues[0]?.path).toEqual([
        "roles",
        "query_embedding",
        "routes",
        0,
      ]);
  });
  it("bounds prompt decoding and canonical path while tracking changed content", async () => {
    const dir = await mkdtemp(join(tmpdir(), "nous-prompts-"));
    temporary.push(dir);
    const root = join(dir, "prompts");
    await mkdir(root);
    await writeFile(join(root, "p.md"), "first");
    await writeFile(join(dir, "outside.md"), "outside");
    const registry = new PromptRegistry(root);
    const first = await registry.load("memory_formation", "p.md");
    await writeFile(join(root, "p.md"), "second");
    expect(
      (await registry.load("memory_formation", "prompts/p.md"))?.digest,
    ).not.toBe(first?.digest);
    await expect(
      registry.load("memory_formation", "../outside.md"),
    ).rejects.toThrow("allowed root");
    await writeFile(join(root, "p.md"), Buffer.alloc(128 * 1024 + 1));
    await expect(registry.load("memory_formation", "p.md")).rejects.toThrow(
      "128 KiB",
    );
    await writeFile(join(root, "p.md"), Buffer.from([0xff]));
    await expect(registry.load("memory_formation", "p.md")).rejects.toThrow();
  });
  it("uses explicit chat/embedding/rerank endpoints and redacts provider errors", async () => {
    const requests: { path: string; body: Record<string, unknown> }[] = [];
    const server = createServer((req, res) => {
      void (async () => {
        const chunks: Buffer[] = [];
        for await (const chunk of req) {
          const data: unknown = chunk;
          if (!(data instanceof Uint8Array))
            throw new Error("Unexpected request chunk");
          chunks.push(Buffer.from(data));
        }
        const body = JSON.parse(Buffer.concat(chunks).toString()) as Record<
          string,
          unknown
        >;
        requests.push({ path: req.url!, body });
        res.setHeader("Content-Type", "application/json");
        if (req.url === "/v1/chat/completions")
          res.end(
            JSON.stringify({
              id: "fixture",
              created: 1,
              model: "chat-id",
              choices: [
                {
                  index: 0,
                  message: {
                    role: "assistant",
                    content:
                      '{"text":"faithful","semanticRole":"reported_fact","title":null,"selectedEntityKeys":[]}',
                  },
                  finish_reason:
                    body.model === "incomplete-chat" ? "length" : "stop",
                },
              ],
              usage: {
                prompt_tokens: 1,
                completion_tokens: 2,
                total_tokens: 3,
              },
            }),
          );
        else if (req.url === "/v1/embeddings")
          res.end(
            JSON.stringify({
              data: [{ object: "embedding", index: 0, embedding: [0.2, 0.3] }],
              model: "embedding-id",
              usage: { prompt_tokens: 1, total_tokens: 1 },
            }),
          );
        else if (body.model === "bad-rerank") {
          res.statusCode = 401;
          res.end('{"error":"fixture-secret echoed by provider"}');
        } else if (body.model === "duplicate-rerank")
          res.end(
            '{"results":[{"index":0,"relevance_score":1},{"index":0,"relevance_score":0}]}',
          );
        else if (body.model === "out-of-range-rerank")
          res.end('{"results":[{"index":2,"relevance_score":1}]}');
        else if (body.model === "nonfinite-rerank")
          res.end('{"results":[{"index":0,"relevance_score":1e400}]}');
        else
          res.end(
            JSON.stringify({ results: [{ index: 1, relevance_score: 0.8 }] }),
          );
      })().catch(() => {
        res.destroy();
      });
    });
    await new Promise<void>((done) => server.listen(0, "127.0.0.1", done));
    const address = server.address();
    if (!address || typeof address === "string")
      throw new Error("Missing fixture port");
    process.env.NOUS_TEST_GATEWAY = "fixture-secret";
    try {
      const config = modelConfigurationSchema.parse({
        gateway_profiles: {
          fixture: {
            base_url: `http://127.0.0.1:${address.port}/v1`,
            credential_env: "NOUS_TEST_GATEWAY",
          },
        },
        model_profiles: {
          chat: {
            gateway: "fixture",
            protocol: "openai-chat",
            model: "chat-id",
            capabilities: [
              "text",
              "image_input",
              "structured_output",
              "audio_input",
              "video_input",
            ],
          },
          embedding: {
            gateway: "fixture",
            protocol: "openai-embeddings",
            model: "embedding-id",
            capabilities: ["embedding"],
            embedding: {
              dimension: 2,
              weights_revision: "1",
              task: "retrieval",
              input_representation: "text",
              preprocessing_identity: "utf8",
              preprocessing_revision: "1",
              normalization: "none",
              output_semantics: "dense",
            },
          },
          rerank: {
            gateway: "fixture",
            protocol: "rerank-v1",
            model: "rank-id",
            capabilities: ["rerank"],
          },
        },
        execution_profiles: {
          memory_formation: { model: "chat" },
          query_embedding: { model: "embedding" },
          query_rerank: { model: "rerank" },
        },
        roles: {
          memory_formation: { routes: ["memory_formation"] },
          query_embedding: { routes: ["query_embedding"] },
          query_rerank: { routes: ["query_rerank"] },
        },
      });
      const runtime = await ModelInvocations.create(config);
      expect(
        runtime.capabilities
          .filter(
            (c) =>
              c.name === "model.memory_formation" ||
              c.name === "model.query_embedding" ||
              c.name === "model.query_rerank",
          )
          .every((c) => c.state === "READY"),
      ).toBe(true);
      const formation = await runtime.generate("memory_formation", {
        content: "evidence",
      });
      expect(formation.value).toEqual({
        text: "faithful",
        semanticRole: "reported_fact",
        title: null,
        selectedEntityKeys: [],
      });
      expect(formation.producerMetadata.promptDigest).toHaveLength(64);
      const outputContract = providerContractForRole("memory_formation")!;
      expect(formation.producerMetadata.outputSchemaDigest).toBe(
        outputContract.digest,
      );
      const format = requests[0]!.body.response_format as {
        type: string;
        json_schema: { strict: boolean; schema: unknown };
      };
      expect(format.type).toBe("json_schema");
      expect(format.json_schema.strict).toBe(true);
      expect(format.json_schema.schema).toEqual(outputContract.providerSchema);
      expect(
        (await runtime.embeddingBatch(["evidence"], "embedding-id")).value[0],
      ).toEqual([0.2, 0.3]);
      expect(
        (await runtime.rerank("intent", ["a", "b"], 2)).value[0]?.index,
      ).toBe(1);
      expect(requests.map((r) => r.path)).toEqual([
        "/v1/chat/completions",
        "/v1/embeddings",
        "/v1/rerank",
      ]);
      await runtime.generate("memory_formation", {
        content: [
          {
            type: "file",
            data: Uint8Array.of(1, 2, 3),
            mediaType: "image/png",
          },
        ],
      });
      const imageMessages = requests.at(-1)!.body.messages as {
        content: unknown[];
      }[];
      expect(imageMessages.at(-1)?.content).toMatchObject([
        { type: "image_url", image_url: { url: "data:image/png;base64,AQID" } },
      ]);
      for (const mediaType of ["audio/mpeg", "video/mp4"]) {
        const result = await runtime.generate("memory_formation", {
          content: "evidence",
          media: { bytes: Uint8Array.of(1, 2, 3), mediaType },
        });
        expect(result.value).toEqual({
          text: "faithful",
          semanticRole: "reported_fact",
          title: null,
          selectedEntityKeys: [],
        });
        expect(result.producerMetadata.outputSchemaDigest).toBe(
          outputContract.digest,
        );
        expect(requests.at(-1)!.body.response_format).toMatchObject({
          type: "json_schema",
          json_schema: {
            name: outputContract.providerName,
            strict: true,
            schema: outputContract.providerSchema,
          },
        });
        const messages = requests.at(-1)!.body.messages as {
          content: unknown;
        }[];
        expect(messages[1]?.content).toEqual([
          { type: "text", text: "evidence" },
          mediaType === "audio/mpeg"
            ? {
                type: "input_audio",
                input_audio: { data: "AQID", format: "mp3" },
              }
            : {
                type: "video_url",
                video_url: { url: "data:video/mp4;base64,AQID" },
              },
        ]);
      }
      const noAudio = structuredClone(config);
      noAudio.model_profiles.chat!.capabilities = ["text", "structured_output"];
      const admittedRequests = requests.length;
      await expect(
        (await ModelInvocations.create(noAudio)).generate("memory_formation", {
          content: "evidence",
          media: { bytes: Uint8Array.of(1, 2, 3), mediaType: "audio/mpeg" },
        }),
      ).rejects.toThrow("audio_input_unavailable");
      expect(requests).toHaveLength(admittedRequests);
      for (const model of [
        "bad-rerank",
        "duplicate-rerank",
        "out-of-range-rerank",
        "nonfinite-rerank",
      ]) {
        config.model_profiles.rerank!.model = model;
        await expect(
          (await ModelInvocations.create(config)).rerank(
            "intent",
            ["a", "b"],
            2,
          ),
        ).rejects.toThrow("all_execution_routes_failed");
      }
      expect(JSON.stringify(formation.producerMetadata)).not.toContain(
        "fixture-secret",
      );
      config.model_profiles.chat!.model = "incomplete-chat";
      const incomplete = await ModelInvocations.create(config);
      await expect(
        incomplete.generate("memory_formation", { content: "evidence" }),
      ).rejects.toThrow("output_incomplete_length");
      expect(
        incomplete.capabilities.find((c) => c.name === "model.memory_formation")
          ?.state,
      ).toBe("READY");
      await expect(
        incomplete.generate("memory_formation", {
          content: "evidence",
          media: { bytes: Uint8Array.of(1), mediaType: "audio/mpeg" },
        }),
      ).rejects.toThrow("response_validation");
      config.model_profiles.backup = {
        ...config.model_profiles.chat!,
        model: "chat-id",
        reasoning_levels: ["provider-default", "high"],
      };
      config.execution_profiles.backup = {
        model: "backup",
        reasoning: "high",
        max_output_tokens: 64,
        provider_options: {},
      };
      config.roles.memory_formation = {
        routes: ["memory_formation", "backup"],
        requirement: "required",
      };
      const routed = await ModelInvocations.create(config);
      const frozen = routed.snapshot("memory_formation");
      const staleContract = { ...frozen, outputSchemaDigest: "0".repeat(64) };
      const beforeContractCheck = requests.length;
      await expect(
        routed.generate(
          "memory_formation",
          { content: "evidence" },
          {
            snapshot: staleContract,
            beforeAttempt: () => {
              throw new Error("contract check must precede admission");
            },
          },
        ),
      ).rejects.toThrow("all_execution_routes_failed:route_unavailable");
      expect(requests.length).toBe(beforeContractCheck);
      config.model_profiles.backup.model = "changed-after-reservation";
      let admitted = 0;
      const start = requests.length;
      const fallback = await routed.generate(
        "memory_formation",
        { content: "evidence" },
        {
          snapshot: frozen,
          beforeAttempt: () => {
            admitted++;
          },
        },
      );
      expect(admitted).toBe(2);
      expect(
        requests.slice(start).map((request) => request.body.model),
      ).toEqual(["incomplete-chat", "chat-id"]);
      expect(requests.at(-1)!.body.reasoning_effort).toBe("high");
      expect(
        fallback.execution.attempts.map((attempt) => attempt.status),
      ).toEqual(["failed", "succeeded"]);
      expect(fallback.producerMetadata).toMatchObject({
        model: "chat-id",
        modelProfile: "backup",
        executionProfile: "backup",
        modelRole: "memory_formation",
      });
      const semanticSnapshot = structuredClone(frozen);
      semanticSnapshot.configuration.model_profiles.chat!.model = "chat-id";
      let semanticChecks = 0;
      const semanticFallback = await routed.generate(
        "memory_formation",
        { content: "evidence" },
        {
          snapshot: semanticSnapshot,
          validateOutput: () => {
            if (++semanticChecks === 1)
              throw new Error("source semantics invalid");
          },
        },
      );
      expect(semanticChecks).toBe(2);
      expect(
        semanticFallback.execution.attempts.map(
          (attempt) => attempt.failureClass,
        ),
      ).toEqual(["output_semantics_invalid", undefined]);
      expect(semanticFallback.producerMetadata.executionProfile).toBe("backup");
      const beforeBudget = requests.length;
      let admission = 0;
      let admissionFailure: unknown;
      try {
        await routed.generate(
          "memory_formation",
          { content: "evidence" },
          {
            snapshot: frozen,
            beforeAttempt: () => {
              if (++admission > 1)
                throw new Error("caller model budget exhausted");
            },
          },
        );
      } catch (error) {
        admissionFailure = error;
      }
      expect(admissionFailure).toBeInstanceOf(Error);
      expect((admissionFailure as Error).message).toBe(
        "caller model budget exhausted",
      );
      expect(failedExecutionTelemetry(admissionFailure)?.attempts).toHaveLength(
        1,
      );
      expect(fallback.execution.attempts.at(-1)?.usage).toMatchObject({
        inputTokens: 1,
        outputTokens: 2,
        totalTokens: 3,
      });
      expect(requests.length - beforeBudget).toBe(1);
    } finally {
      delete process.env.NOUS_TEST_GATEWAY;
      await new Promise<void>((done) => server.close(() => done()));
    }
  });
});

// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { Code, ConnectError } from "@connectrpc/connect";
import { createWritableIterable } from "@connectrpc/connect/protocol";
import { create, toBinary } from "@bufbuild/protobuf";
import { AnySchema } from "@bufbuild/protobuf/wkt";
import {
  domainError,
  domainErrorDetail,
  ErrorCode,
  ErrorOwner,
  ErrorRecovery,
} from "@nous-wave/client/errors";
import { executionOptions, type ExecutionOptions } from "../execution.js";
import {
  QueryEmbeddingResultSchema,
  QueryEmbeddingSchema,
  QueryConceptResultSchema,
  QueryFinalizeResultSchema,
  QueryHostPlanSchema,
  QueryHostRoleSchema,
  QueryRerankOrderSchema,
  QueryRerankResultSchema,
  QueryRunStartSchema,
  QueryRunRequestSchema,
  EmbeddingNeedsRequestSchema,
  CommitEmbeddingRequestSchema,
  QueryHostRequirement,
  type QueryRunRequest,
  type QueryRunUpdate,
} from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/model_pb.js";
import { StatusSchema } from "@nous-wave/protocol/google/rpc/status_pb.js";
import type { QueryRequest } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { QueryConceptOutputSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { ErrorDetailSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/errors_pb.js";
import type { KernelClient } from "../kernel-client.js";
import {
  ModelMaterialPipeline,
  type EmbeddingMaterialChannel,
} from "../model/material.js";
import { ModelRuntime } from "../model/runtime.js";
import type { ResourceRegistry } from "../resources/registry.js";
import { executeResourceActions } from "../resources/execute.js";
import { canonicalDigest } from "../digest.js";

interface CachedQueryVector {
  subjectId: string;
  vector: number[];
}

export class QueryOrchestrator {
  private readonly queryVectors = new Map<string, CachedQueryVector>();
  private readonly queryVectorGenerations = new Map<string, number>();

  constructor(
    private readonly kernel: KernelClient,
    private readonly models: ModelRuntime,
    private readonly resources: ResourceRegistry,
  ) {}

  invalidateSubject(subjectId: string): void {
    this.queryVectorGenerations.set(
      subjectId,
      (this.queryVectorGenerations.get(subjectId) ?? 0) + 1,
    );
    for (const [key, entry] of this.queryVectors)
      if (entry.subjectId === subjectId) this.queryVectors.delete(key);
  }

  async execute(input: QueryRequest, options: ExecutionOptions = {}) {
    const calls = executionOptions(this.kernel.execution.opportunity, options);
    const generation = this.queryVectorGenerations.get(input.subjectId) ?? 0;
    const host = create(QueryHostPlanSchema, {
      embedding: create(QueryHostRoleSchema, {
        configured: Boolean(this.models.embeddingModel),
        requirement:
          this.models.invocations.requirement("query_embedding") === "required"
            ? QueryHostRequirement.REQUIRED
            : QueryHostRequirement.OPTIONAL,
      }),
      conceptEnrichment: create(QueryHostRoleSchema, {
        configured: Boolean(
          this.models.invocations.profile("query_concept_enrichment"),
        ),
        requirement:
          this.models.invocations.requirement("query_concept_enrichment") ===
          "required"
            ? QueryHostRequirement.REQUIRED
            : QueryHostRequirement.OPTIONAL,
      }),
      rerank: create(QueryHostRoleSchema, {
        configured: Boolean(this.models.invocations.profile("query_rerank")),
        requirement:
          this.models.invocations.requirement("query_rerank") === "required"
            ? QueryHostRequirement.REQUIRED
            : QueryHostRequirement.OPTIONAL,
      }),
      rerankCandidateLimit: this.kernel.execution.query_rerank_candidate_limit,
    });

    const requests = createWritableIterable<QueryRunRequest>();
    const workflowTimeoutMs =
      calls.timeoutMs ?? calls.opportunity.policy.work_timeout_ms;
    const stream = this.kernel.queryWorkflow.runQuery(requests, {
      ...calls,
      timeoutMs: workflowTimeoutMs,
    });
    const responses = stream[Symbol.asyncIterator]();
    const send = (frame: QueryRunRequest["frame"]) =>
      requests.write(create(QueryRunRequestSchema, { frame }));
    const nextUpdate = async (): Promise<QueryRunUpdate> => {
      if (calls.signal.aborted) throw calls.signal.reason;
      const next = await responses.next();
      if (next.done)
        throw new ConnectError(
          "Query workflow stream ended early",
          Code.Internal,
        );
      return next.value;
    };
    const exchange = async (frame: QueryRunRequest["frame"]) => {
      const response = nextUpdate();
      try {
        await send(frame);
        return await response;
      } catch (error) {
        await response.catch(() => {});
        throw error;
      }
    };
    try {
      let update = await exchange({
        case: "start",
        value: create(QueryRunStartSchema, { query: input, host }),
      });
      const materialChannel: EmbeddingMaterialChannel = {
        listEmbeddingNeeds: async (request, _options) => {
          const response = await exchange({
            case: "embeddingNeeds",
            value: create(EmbeddingNeedsRequestSchema, request),
          });
          if (response.frame.case !== "embeddingNeeds")
            throw new ConnectError(
              "Historical embedding page response is missing",
              Code.Internal,
            );
          return response.frame.value;
        },
        commitEmbedding: async (request, _options) => {
          const response = await exchange({
            case: "commitEmbedding",
            value: create(CommitEmbeddingRequestSchema, request),
          });
          if (response.frame.case !== "embeddingCommitted")
            throw new ConnectError(
              "Historical embedding commit acknowledgement is missing",
              Code.Internal,
            );
          return response.frame.value;
        },
      };
      while (update.frame.case !== "complete") {
        if (calls.signal.aborted) throw calls.signal.reason;

        if (update.frame.case === "embedding") {
          const work = update.frame.value;
          let outcome;
          try {
            if (!this.models.embeddingModel)
              throw domainError(
                "Query embedding role unavailable",
                Code.Unavailable,
                {
                  code: ErrorCode.CAPABILITY_UNAVAILABLE,
                  owner: ErrorOwner.CORE,
                  recovery: ErrorRecovery.CHECK_CONFIGURATION,
                  context: { capability: "model.query_embedding" },
                },
              );
            if (work.historicalView) {
              const prepared = await new ModelMaterialPipeline(
                this.kernel,
                this.models,
              ).prepare(input.subjectId, 256, calls, materialChannel);
              if (prepared.degradation.length)
                throw domainError(
                  "Historical embedding material unavailable",
                  Code.Unavailable,
                  {
                    code: ErrorCode.UNAVAILABLE,
                    owner: ErrorOwner.CORE,
                    recovery: ErrorRecovery.UNSPECIFIED,
                    context: { capability: "historical_query_embedding" },
                  },
                );
            }
            const config = work.config;
            if (!config)
              throw domainError(
                "Query embedding configuration unavailable",
                Code.FailedPrecondition,
                {
                  code: ErrorCode.FAILED_PRECONDITION,
                  owner: ErrorOwner.KERNEL,
                  recovery: ErrorRecovery.CHECK_CONFIGURATION,
                  context: { capability: "query_embedding_space" },
                },
              );
            const cachedProducer = config.producerHashes.find((producer) => {
              const key = canonicalDigest({
                subject: input.subjectId,
                generation,
                text: work.representation,
                space: config.spaceHash,
                producer,
              });
              return (
                (this.queryVectorGenerations.get(input.subjectId) ?? 0) ===
                  generation && this.queryVectors.has(key)
              );
            });
            const cachedKey =
              cachedProducer &&
              canonicalDigest({
                subject: input.subjectId,
                generation,
                text: work.representation,
                space: config.spaceHash,
                producer: cachedProducer,
              });
            const cached = cachedKey
              ? this.queryVectors.get(cachedKey)
              : undefined;
            const vectors =
              cached && cachedProducer
                ? {
                    producer: { signature_hash: cachedProducer },
                    value: [cached.vector],
                  }
                : await this.models.invocations.embeddingBatch(
                    [work.representation],
                    config.model,
                    calls.signal,
                  );
            if (
              !config.producerHashes.includes(vectors.producer.signature_hash)
            )
              throw domainError(
                "Query embedding producer is not authorized",
                Code.FailedPrecondition,
                {
                  code: ErrorCode.FAILED_PRECONDITION,
                  owner: ErrorOwner.CORE,
                  recovery: ErrorRecovery.CHECK_CONFIGURATION,
                  context: { capability: "query_embedding_producer" },
                },
              );
            const key = canonicalDigest({
              subject: input.subjectId,
              generation,
              text: work.representation,
              space: config.spaceHash,
              producer: vectors.producer.signature_hash,
            });
            if (
              (this.queryVectorGenerations.get(input.subjectId) ?? 0) ===
              generation
            ) {
              this.queryVectors.set(key, {
                subjectId: input.subjectId,
                vector: vectors.value[0]!,
              });
              while (
                this.queryVectors.size >
                this.kernel.execution.query_embedding_cache_entries
              ) {
                const oldest = this.queryVectors.keys().next().value;
                if (oldest === undefined) break;
                this.queryVectors.delete(oldest);
              }
            }
            outcome = {
              case: "material" as const,
              value: create(QueryEmbeddingSchema, {
                text: work.representation,
                spaceHash: config.spaceHash,
                producerHash: vectors.producer.signature_hash,
                vector: vectors.value[0]!,
              }),
            };
          } catch (error) {
            if (calls.signal.aborted) throw error;
            outcome = {
              case: "failure" as const,
              value: rpcFailure(error, "model.query_embedding"),
            };
          }
          update = await exchange({
            case: "embedding",
            value: create(QueryEmbeddingResultSchema, { outcome }),
          });
        } else if (update.frame.case === "concept") {
          const work = update.frame.value;
          let outcome;
          let modelCalls = 0;
          try {
            if (!this.models.invocations.profile("query_concept_enrichment"))
              throw domainError(
                "Query concept model role unavailable",
                Code.Unavailable,
                {
                  code: ErrorCode.CAPABILITY_UNAVAILABLE,
                  owner: ErrorOwner.CORE,
                  recovery: ErrorRecovery.CHECK_CONFIGURATION,
                  context: { capability: "model.query_concept_enrichment" },
                },
              );
            const response = await this.models.invocations.generate(
              "query_concept_enrichment",
              {
                content: work.representation,
                context: () => ({
                  existing_tags: work.existingTags,
                  temporal_frame: work.temporalFrame,
                }),
              },
              {
                signal: calls.signal,
                beforeAttempt: () => {
                  modelCalls++;
                },
              },
            );
            outcome = {
              case: "output" as const,
              value: create(QueryConceptOutputSchema, {
                existingTags: response.value.existing_tags.map((tag) => ({
                  key: tag.key,
                  strength: tag.strength,
                })),
                novelConcepts: response.value.novel_concepts.map((item) => ({
                  text: item.text,
                })),
              }),
            };
          } catch (error) {
            if (calls.signal.aborted) throw error;
            outcome = {
              case: "failure" as const,
              value: rpcFailure(error, "model.query_concept_enrichment"),
            };
          }
          update = await exchange({
            case: "concept",
            value: create(QueryConceptResultSchema, { outcome, modelCalls }),
          });
        } else if (update.frame.case === "finalize") {
          const work = update.frame.value;
          const profile = this.models.invocations.profile("query_rerank");
          const candidates = work.candidates.filter(
            (candidate) => candidate.reference,
          );
          let rerank;
          if (candidates.length >= 2 && profile) {
            try {
              const ranking = await this.models.invocations.rerank(
                work.rerankQuery,
                candidates.map((candidate) => candidate.text),
                candidates.length,
                calls.signal,
              );
              rerank = create(QueryRerankResultSchema, {
                outcome: {
                  case: "order",
                  value: create(QueryRerankOrderSchema, {
                    candidates: ranking.value.map((item) => ({
                      reference: candidates[item.index]!.reference,
                      score: item.relevance_score,
                    })),
                  }),
                },
              });
            } catch (error) {
              if (calls.signal.aborted) throw error;
              rerank = create(QueryRerankResultSchema, {
                outcome: {
                  case: "failure",
                  value: rpcFailure(error, "model.query_rerank"),
                },
              });
            }
          } else {
            rerank = create(QueryRerankResultSchema, {
              outcome: { case: "notRequested", value: true },
            });
          }
          const requiredRerankUnavailable =
            work.candidates.length >= 2 &&
            work.rerankRequirement === QueryHostRequirement.REQUIRED;
          const resources =
            requiredRerankUnavailable &&
            (rerank.outcome.case === "failure" ||
              rerank.outcome.case === "notRequested")
              ? []
              : await executeResourceActions(
                  this.resources,
                  work.resourceActions,
                  calls.signal,
                );
          update = await exchange({
            case: "finalize",
            value: create(QueryFinalizeResultSchema, { rerank, resources }),
          });
        } else {
          throw new ConnectError(
            "Unexpected query workflow frame",
            Code.Internal,
          );
        }
      }
      requests.close();
      await responses.next();
      return update.frame.value;
    } catch (error) {
      await responses.return?.().catch(() => {});
      throw error;
    } finally {
      requests.close();
    }
  }
}

function rpcFailure(error: unknown, capability: string) {
  const connect =
    error instanceof ConnectError
      ? error
      : domainError("Query host model step unavailable", Code.Unavailable, {
          code: ErrorCode.UNAVAILABLE,
          owner: ErrorOwner.CORE,
          recovery: ErrorRecovery.CHECK_CONFIGURATION,
          context: { capability },
        });
  const detail = domainErrorDetail(connect);
  return create(StatusSchema, {
    code: detail ? connect.code : Code.Unavailable,
    message: detail ? connect.message : "Query host model step unavailable",
    details: detail
      ? [
          create(AnySchema, {
            typeUrl: "type.googleapis.com/nous.wave.v1alpha1.ErrorDetail",
            value: toBinary(ErrorDetailSchema, detail),
          }),
        ]
      : [],
  });
}

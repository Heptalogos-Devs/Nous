// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { Code, ConnectError } from "@connectrpc/connect";
import {
  domainError,
  DomainErrorCode,
  ErrorRecovery,
} from "@nous-wave/client/errors";
import { executionOptions, type ExecutionOptions } from "../execution.js";
import { create } from "@bufbuild/protobuf";
import {
  QueryEmbeddingSchema,
  type QueryEmbedding,
} from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/model_pb.js";
import type {
  QueryExpr,
  QueryRequest,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import type { KernelClient } from "../kernel-client.js";
import { ModelMaterialPipeline } from "../model/material.js";
import { ModelRuntime } from "../model/runtime.js";
import { canonicalDigest } from "../digest.js";

import type { ResourceRegistry } from "../resources/registry.js";
import { executeResourceActions } from "../resources/execute.js";
export class QueryOrchestrator {
  private readonly queryVectors = new Map<string, number[]>();
  constructor(
    private readonly kernel: KernelClient,
    private readonly models: ModelRuntime,
    private readonly resources: ResourceRegistry,
  ) {}
  async execute(input: QueryRequest, options: ExecutionOptions = {}) {
    const calls = executionOptions(this.kernel.execution.opportunity, options);
    options = calls;
    const { opportunity } = calls;
    const material: QueryEmbedding[] = [];
    let failure: string | undefined;
    const preparation = await this.kernel.queryWorkflow.prepareQuery(
      {
        query: input,
        reserveExecution: true,
        leaseSeconds: opportunity.leaseSeconds,
      },
      options,
    );
    if (!preparation.preparationToken)
      throw new ConnectError("Prepared query token missing", Code.Internal);
    let executionToken = preparation.preparationToken;
    try {
      const texts = new Set(
        preparation.embeddingRequired && preparation.embeddingText
          ? [preparation.embeddingText]
          : [],
      );
      if (texts.size) {
        try {
          if (!this.models.embeddingModel)
            throw domainError(
              "Query embedding role unavailable",
              Code.FailedPrecondition,
              {
                code: DomainErrorCode.CAPABILITY_UNAVAILABLE,
                recovery: ErrorRecovery.CHECK_CONFIGURATION,
                context: { capability: "model.query_embedding" },
              },
            );
          if (preparation.historicalView) {
            const prepared = await new ModelMaterialPipeline(
              this.kernel,
              this.models,
            ).prepare(input.subjectId, 256, options, executionToken);
            if (prepared.degradation.length)
              throw new ConnectError(
                "Historical embedding material unavailable",
                Code.FailedPrecondition,
              );
          }
          const config = await this.kernel.materialWorkflow.getEmbeddingConfig(
            {},
            options,
          );
          const producers = config.producerHashes;
          const key = (text: string, producer: string) =>
            canonicalDigest({ text, space: config.spaceHash, producer });
          const cached = (text: string) =>
            producers
              .map((producer) => ({
                producer,
                vector: this.queryVectors.get(key(text, producer)),
              }))
              .find((item) => item.vector);
          const missing = [...texts].filter((text) => !cached(text));
          if (missing.length) {
            const vectors = await this.models.invocations.embeddingBatch(
              missing,
              config.model,
              options.signal ?? undefined,
            );
            if (!producers.includes(vectors.producer.signature_hash))
              throw new ConnectError(
                "Embedding producer is not in the prepared configuration",
                Code.FailedPrecondition,
              );
            missing.forEach((text, index) =>
              this.queryVectors.set(
                key(text, vectors.producer.signature_hash),
                vectors.value[index]!,
              ),
            );
          }
          for (const text of texts) {
            const selected = cached(text);
            if (!selected?.vector)
              throw new ConnectError(
                "Query embedding material missing",
                Code.Internal,
              );
            material.push(
              create(QueryEmbeddingSchema, {
                text,
                spaceHash: config.spaceHash,
                producerHash: selected.producer,
                vector: selected.vector,
              }),
            );
          }
          while (
            this.queryVectors.size >
            this.kernel.execution.query_embedding_cache_entries
          )
            this.queryVectors.delete(this.queryVectors.keys().next().value!);
        } catch (error) {
          if (options.signal?.aborted) throw error;
          if (
            preparation.textEmbeddingRequirement === "required" ||
            this.models.invocations.requirement("query_embedding") ===
              "required"
          )
            throw new ConnectError(
              "Required query embedding unavailable",
              Code.FailedPrecondition,
            );
          failure =
            error instanceof Error ? error.message : "Embedding unavailable";
        }
      }
      let conceptOutput: string | undefined;
      let conceptFailure: string | undefined;
      let conceptModelCalls = 0;
      if (
        preparation.conceptEnrichmentMode === "model" &&
        preparation.conceptEnrichmentRequirement !== "forbidden"
      ) {
        const activation = await this.kernel.queryWorkflow.activateQuery(
          {
            subjectId: input.subjectId,
            preparationToken: executionToken,
            embeddings: material,
          },
          options,
        );
        executionToken = activation.preparationToken;
        if (!executionToken)
          throw new ConnectError(
            "Query activation token missing",
            Code.Internal,
          );
        try {
          if (!this.models.invocations.profile("query_concept_enrichment"))
            throw domainError(
              "Query concept model role unavailable",
              Code.FailedPrecondition,
              {
                code: DomainErrorCode.CAPABILITY_UNAVAILABLE,
                recovery: ErrorRecovery.CHECK_CONFIGURATION,
                context: { capability: "model.query_concept_enrichment" },
              },
            );
          const response = await this.models.invocations.generate(
            "query_concept_enrichment",
            { content: activation.modelInput },
            {
              signal: options.signal ?? undefined,
              beforeAttempt: () => {
                conceptModelCalls++;
              },
            },
          );
          conceptOutput = JSON.stringify(response.value);
        } catch (error) {
          if (options.signal?.aborted) throw error;
          if (
            preparation.conceptEnrichmentRequirement === "required" ||
            this.models.invocations.requirement("query_concept_enrichment") ===
              "required"
          )
            throw new ConnectError(
              "Required query concept model unavailable",
              Code.FailedPrecondition,
            );
          conceptFailure =
            error instanceof Error
              ? error.message
              : "Query concept model unavailable";
        }
      }
      const intent = input.expression ? positiveIntent(input.expression) : "";
      const rerankAllowed = preparation.rerankRequirement !== "forbidden";
      const rerankRequired =
        rerankAllowed &&
        (preparation.rerankRequirement === "required" ||
          this.models.invocations.requirement("query_rerank") === "required");
      const profile = rerankAllowed
        ? this.models.invocations.profile("query_rerank")
        : undefined;
      if (intent && !profile && rerankRequired)
        throw domainError(
          "Required query rerank role unavailable",
          Code.FailedPrecondition,
          {
            code: DomainErrorCode.CAPABILITY_UNAVAILABLE,
            recovery: ErrorRecovery.CHECK_CONFIGURATION,
            context: { capability: "model.query_rerank" },
          },
        );
      const prepared = await this.kernel.queryWorkflow.query(
        {
          subjectId: input.subjectId,
          preparationToken: executionToken,
          embeddings: conceptOutput || conceptFailure ? [] : material,
          conceptOutput,
          conceptFailure,
          conceptModelCalls,
          validatedCandidateLimit:
            intent && profile
              ? this.kernel.execution.query_rerank_candidate_limit
              : undefined,
        },
        options,
      );
      if (!prepared.response)
        throw new ConnectError("Kernel query response missing", Code.Internal);
      let result = prepared.response;
      const ticket = prepared.validationTicket;
      if (!ticket && intent && profile && rerankRequired)
        throw new ConnectError(
          "Required rerank validation snapshot unavailable",
          Code.ResourceExhausted,
        );
      if (ticket) {
        const candidates = result.hits
          .filter((hit) => hit.text?.trim())
          .slice(0, this.kernel.execution.query_rerank_candidate_limit);
        let order: {
          reference: NonNullable<(typeof candidates)[number]["reference"]>;
          score: number;
        }[] = [];
        let rerankFailed = false;
        try {
          if (intent && profile && candidates.length >= 2) {
            try {
              const ranking = await this.models.invocations.rerank(
                preparation.embeddingText || intent,
                candidates.map((hit) => hit.text!),
                candidates.length,
                options.signal ?? undefined,
              );
              order = ranking.value.map((item) => {
                const reference = candidates[item.index]!.reference;
                if (!reference)
                  throw new ConnectError(
                    "Kernel candidate reference missing",
                    Code.Internal,
                  );
                return { reference, score: item.relevance_score };
              });
            } catch (error) {
              if (options.signal?.aborted) throw error;
              if (rerankRequired)
                throw new ConnectError(
                  "Required rerank invocation unavailable",
                  Code.FailedPrecondition,
                );
              rerankFailed = true;
            }
          }
          result = await this.kernel.queryWorkflow.finalizeQuery(
            {
              subjectId: input.subjectId,
              validationTicket: ticket,
              order,
              externalResults: await executeResourceActions(
                this.resources,
                result.resourceActions,
                options.signal ?? undefined,
              ),
            },
            options,
          );
          if (rerankFailed) {
            result.degradation.push({
              $typeName: "nous.wave.v1alpha1.Degradation",
              code: "query_rerank_unavailable",
              detail: "Validated baseline retained after model rerank failure",
            });
            if (result.status === "complete") result.status = "degraded";
          }
        } finally {
          await this.kernel.queryWorkflow
            .releaseQuery(
              { subjectId: input.subjectId, validationTicket: ticket },
              opportunity.cleanup(),
            )
            .catch(() => {});
        }
      }
      if (failure) {
        result.degradation.push({
          $typeName: "nous.wave.v1alpha1.Degradation",
          code: "query_embedding_unavailable",
          detail: failure,
        });
        if (result.status === "complete") result.status = "degraded";
      }
      result.boundQuery ??= prepared.response.boundQuery;
      return result;
    } finally {
      await this.kernel.queryWorkflow
        .releaseQuery(
          {
            subjectId: input.subjectId,
            validationTicket: executionToken,
          },
          opportunity.cleanup(),
        )
        .catch(() => {});
    }
  }
}
function positiveIntent(node: QueryExpr): string {
  const own = node.cues
    .map((cue) =>
      cue.cue.case === "text" || cue.cue.case === "concept"
        ? cue.cue.value.trim()
        : "",
    )
    .filter(Boolean)
    .join(" ");
  if (node.operation === "atom") return own;
  const children = node.children.map(positiveIntent).filter(Boolean);
  if (!children.length) return "";
  return `${node.operation === "all" ? "ALL OF" : "ANY OF"}: ${children.map((text) => `(${text})`).join("; ")}`;
}

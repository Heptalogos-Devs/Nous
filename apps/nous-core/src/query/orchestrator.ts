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
import { safeProviderFailure } from "../model/protocols.js";
import { GenerationFailure } from "../model/execution/routes.js";
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
    let material: QueryEmbedding | undefined;
    let failure: string | undefined;
    const preparation = await this.kernel.queryWorkflow.prepareQuery(
      {
        query: input,
        executeQuery: true,
        leaseSeconds:
          Math.ceil(
            (opportunity.remainingWorkMs +
              opportunity.policy.cleanup_timeout_ms) /
              1000,
          ) + 1,
      },
      options,
    );
    if (!preparation.preparationToken && !preparation.result)
      throw new ConnectError("Prepared query token missing", Code.Internal);
    let executionToken = preparation.preparationToken;
    try {
      const text = preparation.embeddingRequired
        ? preparation.embeddingText
        : "";
      if (text) {
        try {
          if (!this.models.invocations.profile("query_embedding"))
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
          const key = (sourceText: string, producer: string) =>
            canonicalDigest({
              text: sourceText,
              space: config.spaceHash,
              producer,
            });
          const cached = (sourceText: string) =>
            producers
              .map((producer) => ({
                producer,
                vector: this.queryVectors.get(key(sourceText, producer)),
              }))
              .find((item) => item.vector);
          let selected = cached(text);
          if (!selected?.vector) {
            const vectors = await this.models.invocations.embeddingBatch(
              [text],
              config.model,
              options.signal ?? undefined,
            );
            if (!producers.includes(vectors.producer.signature_hash))
              throw new ConnectError(
                "Embedding producer is not in the prepared configuration",
                Code.FailedPrecondition,
              );
            const vector = vectors.value[0];
            if (!vector)
              throw new ConnectError(
                "Query embedding material missing",
                Code.Internal,
              );
            selected = { producer: vectors.producer.signature_hash, vector };
            this.queryVectors.set(key(text, selected.producer), vector);
          }
          material = create(QueryEmbeddingSchema, {
            text,
            spaceHash: config.spaceHash,
            producerHash: selected.producer,
            vector: selected.vector,
          });
          while (
            this.queryVectors.size >
            this.kernel.execution.query_embedding_cache_entries
          )
            this.queryVectors.delete(this.queryVectors.keys().next().value!);
        } catch (error) {
          this.models.invocations.allowDegradation(
            error,
            options.signal,
            "query_embedding",
            preparation.textEmbeddingRequirement === "required",
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
            preparationToken: executionToken ?? "",
            embedding: material,
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
          this.models.invocations.allowDegradation(
            error,
            options.signal,
            "query_concept_enrichment",
            preparation.conceptEnrichmentRequirement === "required",
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
      const prepared = preparation.result
        ? {
            response: preparation.result,
            validationTicket: preparation.validationTicket,
          }
        : await this.kernel.queryWorkflow.query(
            {
              subjectId: input.subjectId,
              preparationToken: executionToken ?? "",
              embedding: conceptOutput || conceptFailure ? undefined : material,
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
        let rerankFailure: string | undefined;
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
              this.models.invocations.allowDegradation(
                error,
                options.signal,
                "query_rerank",
                rerankRequired,
              );
              rerankFailure =
                error instanceof GenerationFailure
                  ? error.reason
                  : safeProviderFailure(error);
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
          if (rerankFailure) {
            result.degradation.push({
              $typeName: "nous.wave.v1alpha1.Degradation",
              code: "query_rerank_unavailable",
              detail: `Validated baseline retained after model rerank failure: ${rerankFailure}`,
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
      if (executionToken)
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

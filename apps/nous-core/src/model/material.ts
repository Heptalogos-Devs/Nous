// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { Code, ConnectError, type CallOptions } from "@connectrpc/connect";
import type { EmbeddingCursor } from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/model_pb.js";
import type { KernelClient } from "../kernel-client.js";
import { ModelRuntime } from "./runtime.js";
export class ModelMaterialPipeline {
  constructor(
    private readonly kernel: KernelClient,
    private readonly models: ModelRuntime,
  ) {}
  async prepare(
    subjectId: string,
    limit: number,
    options: CallOptions = {},
    preparationToken?: string,
  ) {
    let committed = 0;
    let cursor: EmbeddingCursor | undefined;
    const batchSize =
      this.models.invocations.profile("query_embedding")?.embedding
        ?.max_batch_size ?? 64;
    do {
      const needs = await this.kernel.materialWorkflow.listEmbeddingNeeds(
        { subjectId, limit: limit - committed, preparationToken, cursor },
        options,
      );
      if (!needs.config)
        throw new ConnectError(
          "Embedding configuration unavailable",
          Code.FailedPrecondition,
        );
      for (let offset = 0; offset < needs.needs.length; offset += batchSize) {
        try {
          const batch = needs.needs.slice(offset, offset + batchSize);
          const vectors = await this.models.invocations.embeddingBatch(
            batch.map((need) => need.text),
            needs.config.model,
            options.signal ?? undefined,
          );
          for (const [index, need] of batch.entries()) {
            await this.kernel.materialWorkflow.commitEmbedding(
              {
                subjectId,
                preparationToken,
                reference: need.reference,
                material: {
                  text: need.text,
                  spaceHash: needs.config.spaceHash,
                  producerHash: vectors.producer.signature_hash,
                  vector: vectors.value[index]!,
                },
              },
              options,
            );
            committed++;
          }
        } catch (error) {
          if (options.signal?.aborted) throw error;
          return {
            committed,
            degradation: [
              {
                code: "embedding_batch_incomplete",
                detail:
                  error instanceof Error
                    ? error.message
                    : "Embedding material failed",
              },
            ],
          };
        }
      }
      cursor = needs.nextCursor;
    } while (cursor && committed < limit);
    return { committed, degradation: [] };
  }
}

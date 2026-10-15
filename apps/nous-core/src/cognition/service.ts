// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { create } from "@bufbuild/protobuf";
import { Code, ConnectError, type CallOptions } from "@connectrpc/connect";
import { ResolveIdentityResponseSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/identity_pb.js";
import {
  domainError,
  DomainErrorCode,
  ErrorRecovery,
} from "@nous-wave/client/errors";
import {
  DegradationSchema,
  ManagedContextResponseSchema,
  ProjectionRequestSchema,
  QueryRequestSchema,
  type ManagedContextRequest,
  type ProjectionRequest,
  type QueryRequest,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import type { ConsumerPolicy } from "../domain.js";
import type { executionOptions } from "../execution.js";
import type { KernelClient } from "../kernel-client.js";
import { ModelRuntime } from "../model/runtime.js";
import { compileNousQL } from "../nousql/compiler.js";
import { QueryOrchestrator } from "../query/orchestrator.js";
import { ResourceRegistry } from "../resources/registry.js";
import { ContextCompiler } from "./context.js";
import { ProjectionPlanner } from "./projection.js";
import { bindProjectionUsage } from "./usage.js";

type Execution = ReturnType<typeof executionOptions>;

/** Core cognition composition; the HTTP host owns transport and assembly. */
export class CoreCognition {
  private readonly planner;
  private readonly contexts;
  private readonly queries;

  constructor(
    private readonly kernel: KernelClient,
    models: ModelRuntime,
    resources: ResourceRegistry,
    consumers: readonly ConsumerPolicy[],
  ) {
    this.planner = new ProjectionPlanner(
      new Map(consumers.map((policy) => [policy.consumerId, policy])),
      models,
    );
    this.contexts = new ContextCompiler(kernel.execution.context_track_limit);
    this.queries = new QueryOrchestrator(kernel, models, resources);
  }

  async prepareQuery(request: QueryRequest, calls: Execution) {
    return this.queries.prepare(
      await this.bindQueryInput(request, calls),
      calls,
    );
  }

  async query(request: QueryRequest, calls: Execution) {
    await this.bindQueryInput(request, calls);
    const result = await this.queries.execute(request, calls);
    const references = result.hits.map((hit) => hit.revision ?? hit.reference);
    const targets = new Map(
      references
        .filter((ref) => ref !== undefined)
        .map((canonical) => [
          `${canonical.kind}:${canonical.value}`,
          { subjectId: request.subjectId, canonical },
        ]),
    );
    if (!targets.size) return result;
    try {
      const addresses = await this.kernel.identity.getIdentityAddresses(
        { targets: [...targets.values()] },
        calls,
      );
      const lexical = new Map(
        addresses.addresses
          .filter(
            (address) =>
              address.status === "BOUND" &&
              address.target?.canonical &&
              address.lexicalRef,
          )
          .map((address) => [
            `${address.target!.canonical!.kind}:${address.target!.canonical!.value}`,
            address.lexicalRef!,
          ]),
      );
      result.hits.forEach((hit, index) => {
        const reference = references[index];
        if (reference)
          hit.lexicalRef = lexical.get(`${reference.kind}:${reference.value}`);
      });
    } catch {
      result.degradation.push(
        create(DegradationSchema, {
          code: "address_directory_unavailable",
          detail:
            "Query result retains canonical references; directory addresses are unavailable",
        }),
      );
    }
    return result;
  }

  async buildProjection(input: ProjectionRequest, calls: Execution) {
    const policy = this.planner.policy(input.consumerId);
    const request = create(ProjectionRequestSchema, input);
    bindProjectionUsage(request);
    request.memoryRequirement = policy.memory;
    request.runtimeRequirement = policy.runtime;
    request.resourceRequirement = policy.resource;
    request.materialize = policy.materialize;
    request.maxItems = Math.min(
      request.maxItems || policy.maxItems,
      policy.maxItems,
    );
    request.maxTextBytes = Math.min(
      request.maxTextBytes || policy.maxTextBytes,
      policy.maxTextBytes,
    );
    const queryDegradation = [];
    if (request.query) {
      const query = await this.queries.execute(
        create(QueryRequestSchema, {
          subjectId: request.subjectId,
          sessionId: request.sessionId,
          expression: request.query,
          workContextId: request.activeWorkContextId,
          situation: {
            consumer: request.consumerId,
            currentRefs: request.situationRefs,
          },
        }),
        calls,
      );
      request.situationRefs.push(
        ...query.hits.flatMap((hit) => {
          const reference = hit.revision ?? hit.reference;
          return reference ? [reference] : [];
        }),
      );
      queryDegradation.push(...query.degradation);
      request.query = undefined;
    }
    const batch = await this.kernel.projection.buildContributionBatch(
      request,
      calls,
    );
    batch.degradation.push(...queryDegradation);
    return this.planner.build(batch, request, policy, calls.signal);
  }

  async buildManagedContext(request: ManagedContextRequest, calls: Execution) {
    if (!request.projection)
      throw new ConnectError(
        "Projection request required",
        Code.InvalidArgument,
      );
    const input = request.projection;
    if (!input.usage)
      throw new ConnectError(
        "Managed context requires actual invocation usage",
        Code.InvalidArgument,
      );
    const projection = await this.buildProjection(input, calls);
    const policy = this.planner.policy(input.consumerId);
    return create(
      ManagedContextResponseSchema,
      this.contexts.compile(
        {
          subjectId: projection.subjectId,
          sessionId: projection.sessionId,
          consumerId: projection.consumerId,
          workContextId: projection.activeWorkContextId,
          usage: projection.usage,
        },
        policy.revision,
        projection,
        request.knownCursor,
      ),
    );
  }

  private async bindQueryInput(request: QueryRequest, calls: CallOptions) {
    if (request.nousql !== undefined) {
      if (request.expression)
        throw new ConnectError(
          "Supply either NousQL or typed expression",
          Code.InvalidArgument,
        );
      const cognitiveTime = await this.kernel.queryWorkflow.getCognitiveTime(
        { subjectId: request.subjectId },
        calls,
      );
      const compiled = await compileNousQL(
        request.nousql,
        async (kind, locator, asOf) => {
          const result = await this.kernel.identity.resolveIdentity(
            {
              subjectId: request.subjectId,
              kind,
              asOf,
              locator: {
                case: locator.kind === "name" ? "name" : "lexicalRef",
                value: locator.value,
              },
            },
            calls,
          );
          if (result.status !== "BOUND" || !result.candidates[0]?.canonical) {
            const code = {
              UNKNOWN_REFERENCE: DomainErrorCode.UNKNOWN_REFERENCE,
              AMBIGUOUS_REFERENCE: DomainErrorCode.AMBIGUOUS_REFERENCE,
              REFERENCE_TYPE_MISMATCH: DomainErrorCode.REFERENCE_TYPE_MISMATCH,
              REFERENCE_TOMBSTONED: DomainErrorCode.REFERENCE_TOMBSTONED,
            }[result.status];
            if (!code)
              throw new ConnectError(
                "Invalid identity resolution result",
                Code.Internal,
              );
            const recovery =
              code === DomainErrorCode.AMBIGUOUS_REFERENCE
                ? ErrorRecovery.SELECT_CANDIDATE
                : code === DomainErrorCode.REFERENCE_TOMBSTONED
                  ? ErrorRecovery.REDISCOVER_REFERENCE
                  : code === DomainErrorCode.REFERENCE_TYPE_MISMATCH
                    ? ErrorRecovery.CORRECT_REQUEST
                    : ErrorRecovery.RESOLVE_REFERENCE;
            const error = domainError(
              "Query reference could not be bound",
              Code.InvalidArgument,
              {
                code,
                recovery,
                context: { reference: locator.value, expected_kind: kind },
              },
            );
            error.details.push({
              desc: ResolveIdentityResponseSchema,
              value: result,
            });
            throw error;
          }
          return {
            canonical: result.candidates[0].canonical,
            lexicalRef: result.candidates[0].lexicalRef,
          };
        },
        cognitiveTime,
      );
      request.expression = compiled.expression;
      request.nousql = undefined;
    }
    return request;
  }
}

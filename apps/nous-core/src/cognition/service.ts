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
    return this.kernel.queryWorkflow.prepareQuery(
      {
        query: await this.bindQueryInput(request, calls),
        reserveExecution: false,
      },
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
    const snapshot = await this.kernel.runtime.getSession(
      { subjectId: request.subjectId, id: request.sessionId },
      calls,
    );
    if (
      request.activeWorkContextId &&
      request.activeWorkContextId !== snapshot.activeWorkContextId
    )
      throw domainError(
        "Projection WorkContext is not active",
        Code.FailedPrecondition,
        {
          code: DomainErrorCode.STALE_CONTEXT,
          recovery: ErrorRecovery.REFRESH_STATE,
          context: {
            session_id: request.sessionId,
            work_context_id: request.activeWorkContextId,
          },
        },
      );
    request.activeWorkContextId ??= snapshot.activeWorkContextId;
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
        }),
        calls,
      );
      request.situationRefs.push(
        ...query.hits.flatMap((hit) => (hit.reference ? [hit.reference] : [])),
      );
      queryDegradation.push(...query.degradation);
      request.query = undefined;
    }
    const batch = await this.kernel.projection.buildContributionBatch(
      request,
      calls,
    );
    batch.degradation.push(...queryDegradation);
    const projection = await this.planner.build(batch, request, calls.signal);
    const after = await this.kernel.runtime.getSession(
      { subjectId: request.subjectId, id: request.sessionId },
      calls,
    );
    if (after.runtimeRevision !== snapshot.runtimeRevision)
      throw domainError("Session changed during projection", Code.Aborted, {
        code: DomainErrorCode.STALE_CONTEXT,
        recovery: ErrorRecovery.REFRESH_STATE,
        context: {
          session_id: request.sessionId,
          expected_revision: snapshot.runtimeRevision.toString(),
          actual_revision: after.runtimeRevision.toString(),
        },
      });
    return projection;
  }

  async buildManagedContext(request: ManagedContextRequest, calls: Execution) {
    if (!request.projection)
      throw new ConnectError(
        "Projection request required",
        Code.InvalidArgument,
      );
    const input = request.projection;
    const projection = await this.buildProjection(input, calls);
    const session = await this.kernel.runtime.getSession(
      { subjectId: input.subjectId, id: input.sessionId },
      calls,
    );
    if (session.runtimeRevision !== projection.sourceRuntimeRevision)
      throw domainError(
        "Session changed before context synchronization",
        Code.Aborted,
        {
          code: DomainErrorCode.STALE_CONTEXT,
          recovery: ErrorRecovery.REFRESH_STATE,
          context: {
            session_id: input.sessionId,
            expected_revision: projection.sourceRuntimeRevision.toString(),
            actual_revision: session.runtimeRevision.toString(),
          },
        },
      );
    const policy = this.planner.policy(input.consumerId);
    return create(
      ManagedContextResponseSchema,
      this.contexts.compile(
        {
          subjectId: input.subjectId,
          sessionId: input.sessionId,
          consumerId: input.consumerId,
          workContextId:
            input.activeWorkContextId ?? session.activeWorkContextId,
        },
        `${policy.revision}:${input.maxItems}:${input.maxTextBytes}`,
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

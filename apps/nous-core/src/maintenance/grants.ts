// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { Code, ConnectError, type CallOptions } from "@connectrpc/connect";
import {
  domainErrorDetail,
  DomainErrorCode,
  ErrorRecovery,
} from "@nous-wave/client/errors";
import type {
  MaintenanceGrantRequest,
  MaintenanceOperationResult,
} from "@nous-wave/protocol/nous/wave/v1alpha1/journal_pb.js";
import type { MaintenancePolicy } from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/longitudinal_pb.js";
import type { KernelClient } from "../kernel-client.js";
import type { ModelRuntime } from "../model/runtime.js";
import { canonicalDigest } from "../digest.js";
import {
  GenerationFailure,
  failedExecutionTelemetry,
} from "../model/execution/routes.js";
import { serializeOperation } from "../operation-results.js";
import { authorityRejection } from "./rejection.js";
import { runModelMaintenance } from "./workflow.js";
import { executionOptions } from "../execution.js";

import { maintenanceRoles } from "./records.js";

function problemClass(error: unknown) {
  const detail = domainErrorDetail(error);
  if (detail?.code && DomainErrorCode[detail.code])
    return DomainErrorCode[detail.code]!.toLowerCase();
  return error instanceof ConnectError
    ? `transport_${Code[error.code]?.toLowerCase() ?? "unknown"}`
    : "runtime_failure";
}
// Match the host's existing stderr diagnostics, without error messages or payloads.
function reportFailure(
  subject: string | undefined,
  stage: string,
  kind: string | undefined,
  problem: string,
  decision: string,
  repetitions = 1,
) {
  console.error(
    JSON.stringify({
      event: "maintenance_failure",
      subject,
      stage,
      kind,
      problem,
      decision,
      repetitions,
    }),
  );
}
function retryDelay(policy: MaintenancePolicy, attempt: number) {
  return Math.min(
    policy.retryMaxSeconds,
    policy.retryInitialSeconds * 2 ** Math.min(Math.max(0, attempt - 1), 31),
  );
}

export async function grantMaintenance(
  kernel: KernelClient,
  models: ModelRuntime,
  input: MaintenanceGrantRequest,
  options: CallOptions = {},
) {
  if (
    !input.subjectId ||
    input.maxOperations < 1 ||
    input.maxOperations > 32 ||
    input.maxModelCalls > 32 ||
    input.maxElapsedMs < 1 ||
    input.maxElapsedMs > 900000
  )
    throw new ConnectError(
      "Invalid maintenance opportunity envelope",
      Code.InvalidArgument,
    );
  const calls = executionOptions(
    kernel.execution.opportunity,
    options,
    input.maxElapsedMs,
  );
  return serializeOperation(
    kernel,
    `maintenance:${input.subjectId.toLowerCase()}`,
    calls.signal,
    () =>
      grantMaintenanceOnce(
        kernel,
        models,
        input,
        calls,
        options.signal ?? undefined,
      ),
  );
}

async function grantMaintenanceOnce(
  kernel: KernelClient,
  models: ModelRuntime,
  input: MaintenanceGrantRequest,
  options: import("../execution.js").ExecutionOptions,
  callerSignal?: AbortSignal,
) {
  const started = performance.now();
  const calls = executionOptions(
    kernel.execution.opportunity,
    options,
    input.maxElapsedMs,
  );
  const { signal, opportunity } = calls;
  const policy = await kernel.maintenance.getMaintenancePolicy(
    { subjectId: input.subjectId },
    calls,
  );
  const results: Omit<MaintenanceOperationResult, "$typeName">[] = [];
  let modelCalls = 0;
  if (!policy.enabled)
    return {
      disposition: "disabled_by_policy",
      results,
      modelCalls,
      operationsConsumed: 0,
      elapsedMs: Math.round(performance.now() - started),
    };
  const limit = Math.min(input.maxOperations, policy.maxOperations);
  const readyRoles = new Set(
    models.invocations.capabilities
      .filter((capability) => capability.state === "READY")
      .map((capability) => capability.name),
  );
  const modelExecutionDigest = canonicalDigest(
    Object.fromEntries(
      [...new Set(Object.values(maintenanceRoles))]
        .filter((role) => readyRoles.has(`model.${role}`))
        .map((role) => [role, models.invocations.prepare(role).configDigest]),
    ),
  );
  let operations = 0;
  const visited = new Set<string>();
  let after:
    | import("@nous-wave/protocol/nous/wave/kernel/v1alpha1/longitudinal_pb.js").MaintenanceNeed
    | undefined;
  let stop = false;
  const modelLimit = Math.min(input.maxModelCalls, policy.maxModelCalls);
  while (operations < limit && !signal.aborted && !stop) {
    let needs;
    try {
      needs = await kernel.maintenance.selectMaintenance(
        {
          subjectId: input.subjectId,
          allowedKinds: ["episode_segment", ...Object.keys(maintenanceRoles)],
          modelExecutionDigest,
          limit: 128,
          after,
        },
        calls,
      );
    } catch (error) {
      if (signal.aborted) break;
      throw error;
    }
    if (!needs.needs.length) break;
    for (const need of needs.needs) {
      if (operations >= limit || signal.aborted) break;
      after = need;
      if (visited.has(need.needId)) continue;
      visited.add(need.needId);
      let status: string;
      let problemCode: string | undefined;
      let errorDetail: MaintenanceOperationResult["errorDetail"];
      let nextDue;
      let consumed = need.kind === "subject_reflection" ? 0 : 1;
      try {
        if (need.kind === "episode_segment") {
          const organized = await kernel.maintenance.organizeExperience(
            {
              subjectId: need.subjectId,
              limit: policy.experienceBatchSize,
              close: false,
            },
            calls,
          );
          nextDue = organized.nextDue;
          status = nextDue
            ? "deferred"
            : organized.episodes.length
              ? "committed"
              : "no_change";
        } else {
          const outcome = await runModelMaintenance(
            kernel,
            models,
            need,
            calls,
            modelLimit - modelCalls,
            () => {
              if (modelCalls >= modelLimit)
                throw new ConnectError(
                  "Maintenance model call budget exhausted",
                  Code.ResourceExhausted,
                );
              modelCalls++;
            },
          );
          status = outcome.status;
          if (
            need.kind === "subject_reflection" &&
            "operationsConsumed" in outcome
          )
            consumed = outcome.operationsConsumed;
          problemCode =
            "problemCode" in outcome ? outcome.problemCode : undefined;
          nextDue = "nextDue" in outcome ? outcome.nextDue : undefined;
        }
      } catch (error) {
        if (signal.aborted) {
          // Host opportunity exhaustion leaves the durable need available to the next grant.
          status = "deferred";
          problemCode = callerSignal?.aborted
            ? "opportunity_canceled"
            : "opportunity_budget_exhausted";
        } else if (authorityRejection(error) === "stale") {
          status = "obsolete";
          problemCode = "maintenance_trigger_changed";
        } else if (
          error instanceof ConnectError &&
          error.code === Code.InvalidArgument
        ) {
          status = "rejected_invalid";
          problemCode = "proposal_invalid";
        } else if (
          domainErrorDetail(error)?.code ===
          DomainErrorCode.CAPABILITY_UNAVAILABLE
        ) {
          errorDetail = domainErrorDetail(error);
          status = failedExecutionTelemetry(error)?.attempts.some(
            (a) => a.status !== "skipped",
          )
            ? "retry"
            : "blocked_dependency";
          problemCode = "capability_unavailable";
        } else {
          const retryable =
            domainErrorDetail(error)?.recovery ===
              ErrorRecovery.RETRY_OPERATION ||
            error instanceof GenerationFailure ||
            (error instanceof ConnectError &&
              [
                Code.Unavailable,
                Code.DeadlineExceeded,
                Code.Canceled,
                Code.ResourceExhausted,
              ].includes(error.code));
          status = retryable ? "retry" : "internal_failure";
          problemCode = problemClass(error);
          reportFailure(
            need.subjectId,
            "execute",
            need.kind,
            problemCode,
            status === "internal_failure"
              ? "stop"
              : need.retryCount + 1 >= policy.retryMaxAttempts
                ? "blocked"
                : "retry",
          );
        }
      }
      if (
        (problemCode === "model_unavailable" &&
          need.kind !== "subject_reflection") ||
        problemCode === "model_budget_exhausted"
      )
        continue;
      operations += consumed;
      if (
        need.kind === "subject_reflection" &&
        ["partial", "failed"].includes(status)
      ) {
        problemCode = "reflection_action_rejected";
      }
      if (
        status === "retry" &&
        need.retryCount + 1 >= policy.retryMaxAttempts
      ) {
        status = "blocked_dependency";
        problemCode = "maintenance_retry_exhausted";
      }
      try {
        await kernel.maintenance.finishMaintenance(
          {
            selected: need,
            disposition:
              status === "deferred"
                ? "pending"
                : status === "retry"
                  ? "retry"
                  : status === "blocked_dependency" ||
                      status === "internal_failure" ||
                      (need.kind === "subject_reflection" &&
                        ["partial", "failed"].includes(status))
                    ? "blocked"
                    : status === "obsolete"
                      ? "obsolete"
                      : "satisfied",
            nextDue,
            problemCode,
            retryDelaySeconds:
              status === "retry" ? retryDelay(policy, need.retryCount + 1) : 0,
          },
          opportunity.acknowledge(),
        );
      } catch (error) {
        reportFailure(
          need.subjectId,
          "acknowledge",
          need.kind,
          problemClass(error),
          "pending_work",
        );
        throw error;
      }
      results.push({
        needId: need.needId,
        kind: need.kind,
        status,
        problemCode,
        errorDetail,
      });
      if (status === "internal_failure") {
        stop = true;
        break;
      }
    }
    if (needs.needs.length < 128) break;
  }
  return {
    disposition: signal.aborted
      ? "opportunity_exhausted"
      : results.length
        ? "processed"
        : "no_eligible_work",
    results,
    modelCalls,
    operationsConsumed: operations,
    elapsedMs: Math.round(performance.now() - started),
  };
}

/** Core owns the continuation for standalone opportunities across poll ticks. */
export class SubjectMaintenanceScheduler {
  private pageToken = "";
  private subjects: string[] = [];
  private reachedEnd = false;
  private subject: string | undefined;
  private failures = 0;
  private lastProblem = "";
  constructor(
    private readonly kernel: KernelClient,
    private readonly models: ModelRuntime,
  ) {}

  async poll(signal: AbortSignal) {
    const policy = await this.kernel.maintenance.getMaintenancePolicy(
      { subjectId: "" },
      { signal, timeoutMs: this.kernel.execution.maintenance_rpc_timeout_ms },
    );
    if (!policy.enabled) return policy.pollIntervalSeconds;
    const started = performance.now();
    const tickSignal = AbortSignal.any([
      signal,
      AbortSignal.timeout(policy.maxElapsedMs),
    ]);
    let operations = policy.maxOperations;
    let modelCalls = policy.maxModelCalls;
    const visited = new Set<string>();
    while (operations > 0 && !tickSignal.aborted) {
      if (!this.subjects.length) {
        if (this.reachedEnd) {
          this.pageToken = "";
          this.reachedEnd = false;
        }
        try {
          const page = await this.kernel.subjects.listSubjects(
            { status: "active", page: { pageToken: this.pageToken } },
            {
              signal: tickSignal,
              timeoutMs: this.kernel.execution.maintenance_rpc_timeout_ms,
            },
          );
          this.subjects = page.items
            .filter((subject) => subject.capabilities?.memory)
            .map((subject) => subject.subjectId);
          this.pageToken = page.nextPageToken;
          this.reachedEnd = !page.nextPageToken;
          if (!this.subjects.length) {
            if (this.reachedEnd) break;
            continue;
          }
        } catch (error) {
          if (
            error instanceof ConnectError &&
            error.code === Code.InvalidArgument &&
            this.pageToken
          ) {
            this.pageToken = "";
            this.reachedEnd = false;
            continue;
          }
          throw error;
        }
      }
      const subject = this.subjects[0]!;
      // A full cycle with spare budget waits for the next poll. No Subject gets
      // a second opportunity before the others in this rotation.
      if (visited.has(subject)) break;
      this.subjects.shift();
      visited.add(subject);
      this.subject = subject;
      try {
        const remainingMs = Math.floor(
          policy.maxElapsedMs - (performance.now() - started),
        );
        if (remainingMs <= 0) break;
        const result = await grantMaintenance(
          this.kernel,
          this.models,
          {
            $typeName: "nous.wave.v1alpha1.MaintenanceGrantRequest",
            subjectId: subject,
            maxOperations: 1,
            maxModelCalls: modelCalls,
            maxElapsedMs: remainingMs,
          },
          { signal: tickSignal },
        );
        operations -= result.operationsConsumed;
        modelCalls -= result.modelCalls;
        this.failures = 0;
      } catch (error) {
        if (signal.aborted) break;
        if (error instanceof ConnectError && error.code === Code.NotFound)
          continue;
        this.report(error, "subject_opportunity");
        break; // A failed RPC cannot report its need consumption; stop this tick.
      }
    }
    this.subject = undefined;
    return policy.pollIntervalSeconds;
  }
  report(error: unknown, stage: string) {
    const problem = `${stage}:${problemClass(error)}`;
    this.failures = problem === this.lastProblem ? this.failures + 1 : 1;
    this.lastProblem = problem;
    // Repeated identical failures aggregate at powers of two.
    if ((this.failures & (this.failures - 1)) === 0)
      reportFailure(
        this.subject,
        stage,
        undefined,
        problemClass(error),
        "next_opportunity",
        this.failures,
      );
  }
}

export async function startMaintenanceLoop(
  kernel: KernelClient,
  models: ModelRuntime,
) {
  const stopped = new AbortController();
  const scheduler = new SubjectMaintenanceScheduler(kernel, models);
  let timer: ReturnType<typeof setTimeout> | undefined;
  let active: Promise<void> | undefined;
  const initial = await kernel.maintenance.getMaintenancePolicy(
    { subjectId: "" },
    { timeoutMs: kernel.execution.maintenance_rpc_timeout_ms },
  );
  let pollDelay = initial.pollIntervalSeconds * 1000;
  const tick = async () => {
    let delay = pollDelay;
    try {
      delay = (await scheduler.poll(stopped.signal)) * 1000;
      pollDelay = delay;
    } catch (error) {
      if (!stopped.signal.aborted) scheduler.report(error, "scheduler");
    } finally {
      if (!stopped.signal.aborted) {
        timer = setTimeout(() => {
          active = tick();
        }, delay);
        timer.unref();
      }
    }
  };
  active = tick();
  return async () => {
    stopped.abort();
    if (timer) clearTimeout(timer);
    await active;
  };
}

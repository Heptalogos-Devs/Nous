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
import { GenerationFailure } from "../model/execution/routes.js";
import { runModelMaintenance } from "./workflow.js";
import { timestampFromDate } from "@bufbuild/protobuf/wkt";
import { executionOptions } from "../execution.js";

const roles = {
  episode_resegment: "episode_segmentation",
  journal_review: "journal_synthesis",
  journal_revalidate: "journal_synthesis",
  memory_consolidate: "memory_consolidation",
  concept_maintenance: "concept_maintenance",
} as const;
function allowedKinds(models: ModelRuntime, modelBudget: number) {
  const ready = new Set(
    models.invocations.capabilities
      .filter((role) => role.state === "READY")
      .map((role) => role.name),
  );
  return [
    "episode_segment",
    ...Object.entries(roles)
      .filter(([, role]) => modelBudget > 0 && ready.has(`model.${role}`))
      .map(([kind]) => kind),
  ];
}
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
      elapsedMs: Math.round(performance.now() - started),
    };
  const limit = Math.min(input.maxOperations, policy.maxOperations);
  for (let index = 0; index < limit && !signal.aborted; index++) {
    const needs = await kernel.maintenance.claimMaintenance(
      {
        subjectId: input.subjectId,
        allowedKinds: allowedKinds(models, input.maxModelCalls - modelCalls),
        modelExecutionDigest: canonicalDigest(
          Object.fromEntries(
            Object.values(roles)
              .filter((role) =>
                models.invocations.capabilities.some(
                  (capability) =>
                    capability.name === `model.${role}` &&
                    capability.state === "READY",
                ),
              )
              .map((role) => [
                role,
                models.invocations.snapshot(role).configDigest,
              ]),
          ),
        ),
        limit: 1,
        leaseSeconds: Math.max(
          policy.workerLeaseSeconds,
          opportunity.leaseSeconds,
        ),
      },
      calls,
    );
    const need = needs.needs[0];
    if (!need) break;
    let status: string;
    let problemCode: string | undefined;
    let nextDue;
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
          () => {
            if (modelCalls >= input.maxModelCalls)
              throw new ConnectError(
                "Maintenance model call budget exhausted",
                Code.ResourceExhausted,
              );
            modelCalls++;
          },
        );
        status = outcome.status;
        problemCode =
          "problemCode" in outcome ? outcome.problemCode : undefined;
        nextDue = "nextDue" in outcome ? outcome.nextDue : undefined;
      }
    } catch (error) {
      if (signal.aborted) {
        // Host opportunity exhaustion leaves the durable need available to the next grant.
        status = "deferred";
        nextDue = timestampFromDate(new Date());
        problemCode = options.signal?.aborted
          ? "opportunity_canceled"
          : "opportunity_budget_exhausted";
      } else if (
        error instanceof ConnectError &&
        error.code === Code.InvalidArgument
      ) {
        status = "rejected_invalid";
        problemCode = "proposal_invalid";
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
    if (status === "retry" && need.retryCount + 1 >= policy.retryMaxAttempts) {
      status = "blocked_dependency";
      problemCode = "maintenance_retry_exhausted";
    }
    try {
      await kernel.maintenance.finishMaintenance(
        {
          claimed: need,
          disposition:
            status === "deferred"
              ? "pending"
              : status === "retry"
                ? "retry"
                : status === "blocked_dependency" ||
                    status === "internal_failure"
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
        "lease_recovery",
      );
      throw error;
    }
    results.push({ needId: need.needId, kind: need.kind, status, problemCode });
    if (status === "internal_failure") break;
  }
  return {
    disposition: signal.aborted
      ? "opportunity_exhausted"
      : results.length
        ? "processed"
        : "no_eligible_work",
    results,
    modelCalls,
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
        operations -= result.results.length;
        modelCalls -= result.modelCalls;
        this.failures = 0;
      } catch (error) {
        if (signal.aborted) break;
        if (error instanceof ConnectError && error.code === Code.NotFound)
          continue;
        this.report(error, "subject_opportunity");
        operations--; // Failed execution also consumes this tick's opportunity.
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

// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { create, fromJson, type DescMessage } from "@bufbuild/protobuf";
import { ValueSchema } from "@bufbuild/protobuf/wkt";
import type { Transport } from "@connectrpc/connect";
import { expect, test } from "vitest";
import {
  CognitionUsageSchema,
  ContextSegmentSchema,
  ContextCursorSchema,
  ManagedContextResponseSchema,
  type ManagedContextRequest,
  type ReportUseRequest,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { createNousClient } from "../src/index.js";
import { createManagedBinding } from "../src/managed.js";

function fixture(
  options: Partial<Parameters<typeof createManagedBinding>[0]> = {},
) {
  const observations: unknown[] = [];
  const requests: ManagedContextRequest[] = [];
  const reports: ReportUseRequest[] = [];
  let response = create(ManagedContextResponseSchema, {
    kind: "RESET",
    cursor: { trackId: "track", epochId: "epoch", revision: 1 },
    projection: {
      subjectId: "subject",
      sessionId: "session",
      consumerId: "consumer",
      projectionId: "p",
      usage: create(CognitionUsageSchema, {
        agentInstanceId: "worker",
        purpose: "deliberation",
        intent: "Continue reading",
        audience: { conversationRef: "conversation", visibility: "internal" },
      }),
      segments: [
        {
          segmentId: "a",
          text: "Old conditional understanding",
          sourceRefs: [{ kind: "memory_revision", value: "revision-a" }],
        },
      ],
    },
  });
  const unary = async (
    method: { name: string; output: DescMessage },
    _signal: unknown,
    _timeout: unknown,
    _headers: unknown,
    input: unknown,
  ) => {
    if (method.name === "GetConfiguration")
      return {
        message: create(method.output, {
          entries: Object.entries({
            work_timeout_ms: 300000,
            cleanup_timeout_ms: 10000,
            acknowledgement_timeout_ms: 5000,
            response_margin_ms: 1000,
          }).map(([name, value]) => ({
            path: `core_execution.opportunity.${name}`,
            value: fromJson(ValueSchema, value),
          })),
        }),
      };
    if (method.name === "BuildManagedContext") {
      requests.push(structuredClone(input as ManagedContextRequest));
      return { message: response };
    }
    if (method.name === "ReportUse") {
      reports.push(input as ReportUseRequest);
      return { message: create(method.output, { modelInputChecked: true }) };
    }
    if (method.name === "RecordObservation") observations.push(input);
    return { message: create(method.output) };
  };
  const client = createNousClient({ unary } as unknown as Transport);
  const binding = createManagedBinding({
    client: () => client,
    subjectId: "subject",
    sessionId: "session",
    consumerId: "consumer",
    ...options,
  });
  return {
    observations,
    binding,
    requests,
    reports,
    response: () => response,
    replace: (next: typeof response) => {
      response = next;
    },
    prepare: () =>
      binding.prepare({
        usage: response.projection!.usage!,
        cognitionTokenBudget: 2048,
        signal: new AbortController().signal,
      }),
  };
}

test("APPEND follows the applied cursor; RESET removes the old slice while host history remains unchanged", async () => {
  const current = fixture();
  const history = ["actual user message", "paired tool call/result"];
  const renderer = {
    render: (base: string[], segments: readonly { text: string }[]) => [
      ...base,
      ...segments.map((segment) => segment.text),
    ],
  };
  const first = current.binding.apply(
    history,
    await current.prepare(),
    renderer,
  );
  expect(first.input.slice(2)).toEqual(["Old conditional understanding"]);
  current.replace(
    create(ManagedContextResponseSchema, {
      ...current.response(),
      kind: "APPEND",
      cursor: create(ContextCursorSchema, {
        trackId: "track",
        epochId: "epoch",
        revision: 2,
      }),
      projection: {
        ...current.response().projection!,
        segments: [
          create(ContextSegmentSchema, {
            segmentId: "b",
            text: "Actual tool result understanding",
          }),
        ],
      },
    }),
  );
  const appended = current.binding.apply(
    history,
    await current.prepare(),
    renderer,
  );
  expect(appended.input.slice(2)).toEqual([
    "Old conditional understanding",
    "Actual tool result understanding",
  ]);
  expect(
    current.binding
      .appliedProjection(appended.applied)
      .segments.map((s) => s.text),
  ).toEqual([
    "Old conditional understanding",
    "Actual tool result understanding",
  ]);
  const clone = current.binding.appliedProjection(appended.applied);
  clone.segments.length = 0;
  expect(
    current.binding.appliedProjection(appended.applied).segments,
  ).toHaveLength(2);
  current.replace(
    create(ManagedContextResponseSchema, {
      ...current.response(),
      kind: "RESET",
      cursor: create(ContextCursorSchema, {
        trackId: "track",
        epochId: "new",
        revision: 1,
      }),
      projection: {
        ...current.response().projection!,
        segments: [
          create(ContextSegmentSchema, {
            segmentId: "c",
            text: "Revised understanding with the counterexample",
          }),
        ],
      },
    }),
  );
  expect(
    current.binding
      .apply(history, await current.prepare(), renderer)
      .input.slice(2),
  ).toEqual(["Revised understanding with the counterexample"]);
  expect(history).toEqual(["actual user message", "paired tool call/result"]);
});

test("failed application leaves the cursor uncommitted; closed and foreign preparations cannot apply", async () => {
  const current = fixture();
  expect(() =>
    current.binding.apply("base", current.response(), {
      render: () => "wrong",
    }),
  ).toThrow("pending");
  const prepared = await current.prepare();
  expect(() =>
    current.binding.apply("base", prepared, {
      render: () => {
        throw new Error("render failed");
      },
    }),
  ).toThrow("render failed");
  await current.prepare();
  expect(current.requests.at(-1)?.knownCursor).toBeUndefined();
  current.replace(
    create(ManagedContextResponseSchema, {
      ...current.response(),
      projection: { ...current.response().projection!, subjectId: "foreign" },
    }),
  );
  const foreign = await current.prepare();
  expect(() =>
    current.binding.apply("base", foreign, { render: (base) => base }),
  ).toThrow("foreign scope");
  await current.binding.close();
  expect(() =>
    current.binding.apply("base", foreign, { render: (base) => base }),
  ).toThrow("closed");
});

test("actual presented reports use the internal applied references and stable native invocation identities", async () => {
  const current = fixture();
  const applied = current.binding.apply("base", await current.prepare(), {
    render: (base) => base,
  }).applied;
  applied.sourceRefs[0]!.value = "forged-reference";
  const result = {
    invocationId: "actual-native-model-call",
    applied,
    occurredAt: new Date(1000),
    outcome: "completed" as const,
  };
  await expect(current.binding.report(result)).rejects.toThrow("admitted");
  await current.binding.admit(result);
  await current.binding.admit(result);
  await current.binding.report(result);
  expect(current.reports[0]?.events[0]?.reference?.value).toBe("revision-a");
  expect(current.reports[0]?.events[0]?.kind).toBe("presented");
  expect(current.reports[0]?.modelInput).toBe(true);
  expect(current.reports[1]?.events[0]?.eventId).toBe(
    current.reports[0]?.events[0]?.eventId,
  );
});

test("execution-only outcomes stay with their RoleRun instead of becoming Subject observations", async () => {
  const retained: unknown[] = [];
  const current = fixture({
    roleScope: {
      role: "subject.execution",
      purpose: "professional_execution",
      view: "worker",
      contactNature: "execution_only",
    },
    retainExecutionOutcome: async (result) => {
      retained.push(result);
    },
  });
  const prepared = await current.prepare();
  const { applied } = current.binding.apply([], prepared, {
    render: (base) => base,
  });
  const result = {
    invocationId: "actual-input",
    applied,
    occurredAt: new Date(),
    outcome: "completed" as const,
    observations: [
      { subjectId: "subject", sessionId: "session", sourceClass: "tool" },
    ],
  };
  await current.binding.admit(result);
  await current.binding.report(result);
  expect(retained).toEqual([result]);
  expect(current.observations).toEqual([]);
});

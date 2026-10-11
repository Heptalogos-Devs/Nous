// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { create, fromJson, type DescMessage } from "@bufbuild/protobuf";
import { ValueSchema } from "@bufbuild/protobuf/wkt";
import { Code, ConnectError, type Transport } from "@connectrpc/connect";
import { expect, test, vi } from "vitest";
import { createNousClient } from "../src/index.js";

test("a granted opportunity can finish cleanup and acknowledgement before the Client deadline", async () => {
  vi.useFakeTimers();
  const unary = vi.fn(
    async (
      method: { name: string; output: DescMessage },
      signal: AbortSignal | undefined,
      timeout: number | undefined,
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
      return new Promise((resolve, reject) => {
        const deadline = setTimeout(
          () =>
            reject(new ConnectError("Caller deadline", Code.DeadlineExceeded)),
          timeout,
        );
        const completed = setTimeout(
          () => {
            clearTimeout(deadline);
            resolve({ message: create(method.output) });
          },
          method.name === "Query" ? 308000 : 128000,
        );
        signal?.addEventListener(
          "abort",
          () => {
            clearTimeout(deadline);
            clearTimeout(completed);
            reject(signal.reason);
          },
          { once: true },
        );
      });
    },
  );
  const client = createNousClient({ unary } as unknown as Transport);
  const request = {
    subjectId: "s",
    maxOperations: 3,
    maxModelCalls: 3,
    maxElapsedMs: 120000,
  };
  try {
    const result = client.cognition
      .grantMaintenance(request)
      .catch((error: unknown) => error);
    await vi.advanceTimersByTimeAsync(128000);
    expect(await result).not.toBeInstanceOf(Error);
    const query = client.cognition
      .query({ subjectId: "s", nousql: "one complete query opportunity" })
      .catch((error: unknown) => error);
    await vi.advanceTimersByTimeAsync(308000);
    expect(await query).not.toBeInstanceOf(Error);
    const shorter = client.cognition
      .grantMaintenance(request, { timeoutMs: 1000 })
      .catch((error: unknown) => error);
    await vi.advanceTimersByTimeAsync(1000);
    expect(await shorter).toMatchObject({ code: Code.DeadlineExceeded });
  } finally {
    vi.useRealTimers();
  }
});

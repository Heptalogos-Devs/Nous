// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { it, expect } from "vitest";
import type { ManagedModelInput } from "./contract.js";
import { invocationBudget } from "./budget.js";
it("bounds the whole native input independently of the model context and reserves the actual provider output", () => {
  const input = {
    reservedOutputTokens: 4096,
    contextTokenBudget: 8192,
    model: { maxTokens: 2048 },
    measureContext: (context: { measured: number }) => context.measured,
  } as unknown as ManagedModelInput;
  const budget = invocationBudget(input, {
    inputTokens: 1200,
    outputTokens: 128,
  });
  const context = (measured: number) =>
    ({ measured }) as unknown as ManagedModelInput["context"];
  expect(budget.outputTokens).toBe(128);
  expect(budget.available(context(1100))).toBe(100);
  expect(() => budget.assertFits(context(1200))).not.toThrow();
  expect(() => budget.assertFits(context(1201))).toThrow(
    "trusted native/Host budget",
  );
  const native = invocationBudget(input, {
    inputTokens: 100000,
    outputTokens: 100000,
  });
  expect(native.outputTokens).toBe(2048);
  expect(() => native.assertFits(context(6144))).not.toThrow();
  expect(() => native.assertFits(context(6145))).toThrow();
});

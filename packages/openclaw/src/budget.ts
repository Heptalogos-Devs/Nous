// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import type { HostTokenBudget } from "@nous-wave/client/host-bridge";
import type { ManagedModelInput } from "./contract.js";
/** Uses the native complete-input estimator; owns no tokenizer or provider registry. */
export function invocationBudget(
  input: ManagedModelInput,
  policy?: HostTokenBudget,
) {
  const outputTokens = Math.min(
    input.reservedOutputTokens,
    input.model.maxTokens,
    policy?.outputTokens ?? input.reservedOutputTokens,
  );
  const contextTokens = input.contextTokenBudget;
  const inputTokens = Math.min(
    contextTokens - outputTokens,
    policy?.inputTokens ?? contextTokens - outputTokens,
  );
  if (
    ![outputTokens, contextTokens, inputTokens].every(
      (value) => Number.isSafeInteger(value) && value > 0,
    )
  )
    throw new Error(
      "Required native/Host input and output budgets are unavailable",
    );
  return {
    outputTokens,
    inputTokens,
    available(context: ManagedModelInput["context"]) {
      return inputTokens - input.measureContext(context);
    },
    assertFits(context: ManagedModelInput["context"]) {
      const tokens = input.measureContext(context);
      if (tokens > inputTokens || tokens + outputTokens > contextTokens)
        throw Object.assign(
          new Error(
            "Complete model input exceeds the trusted native/Host budget",
          ),
          { code: "NOUS_INPUT_WINDOW_EXCEEDED" },
        );
    },
  };
}

// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { cliErrorPayload } from "./agent.js";
import { renderText } from "./output.js";

const flag = (args: readonly string[], name: string) =>
  args.some((arg) => arg === `--${name}` || arg === `--${name}=true`);

export function formatResult(value: unknown, args: readonly string[]) {
  return flag(args, "json") || flag(args, "raw")
    ? JSON.stringify(value, (_, item: unknown) =>
        typeof item === "bigint" ? item.toString() : item,
      )
    : renderText(value);
}

export function formatError(error: unknown, args: readonly string[]) {
  const payload = cliErrorPayload(error);
  if (!flag(args, "json") && !flag(args, "raw") && !flag(args, "developer"))
    payload.candidates = payload.candidates.map((candidate: unknown) => {
      if (
        candidate &&
        typeof candidate === "object" &&
        "lexicalRef" in candidate
      ) {
        const { canonical: _canonical, ...address } = candidate as Record<
          string,
          unknown
        >;
        return address;
      }
      return candidate;
    });
  return formatResult(payload, args);
}

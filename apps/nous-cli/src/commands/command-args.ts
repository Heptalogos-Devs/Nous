// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import type { ArgsDef } from "citty";

/** Options accepted before and after a command path. */
export const commonArgs: ArgsDef = {
  "run-root": { type: "string" },
  "instance-root": { type: "string" },
  consumer: { type: "string" },
  subject: { type: "string" },
  session: { type: "string" },
  "work-context": { type: "string" },
  json: { type: "boolean" },
  raw: { type: "boolean" },
  developer: { type: "boolean" },
};

export const stringArgs = (...names: string[]): ArgsDef =>
  Object.fromEntries(names.map((name) => [name, { type: "string" }]));

export const booleanArgs = (...names: string[]): ArgsDef =>
  Object.fromEntries(names.map((name) => [name, { type: "boolean" }]));

type CommaSeparatedArg = { type: "string"; separator: "," };

/** Declares comma-separated references at the option that consumes them. */
export const commaSeparatedArgs = (...names: string[]): ArgsDef =>
  Object.fromEntries(
    names.map((name) => [name, { type: "string", separator: "," }]),
  ) as Record<string, CommaSeparatedArg>;

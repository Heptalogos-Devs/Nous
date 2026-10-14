// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

// These common consumer options remain forwarded to CLI, but must not conceal
// launcher-owned commands such as runtime/init/config check.
const consumerValueOptions = new Set([
  "--run-root",
  "--instance-root",
  "--state-root",
  "--consumer",
  "--subject",
  "--session",
  "--work-context",
]);
const consumerBooleanOptions = new Set(["--json", "--developer", "--raw"]);
export function launcherCommandOffset(args: readonly string[]) {
  let offset = 0;
  while (offset < args.length) {
    const option = args[offset]!;
    const name = option.split("=")[0]!;
    if (consumerValueOptions.has(name)) offset += option.includes("=") ? 1 : 2;
    else if (consumerBooleanOptions.has(name)) offset++;
    else break;
  }
  return offset;
}

// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { spawn } from "node:child_process";
import { stat } from "node:fs/promises";
import { join, resolve } from "node:path";
import { portableLocator, workspacePaths } from "../workspace.js";

const args = process.argv.slice(2);
let installation = join(workspacePaths.releases, "windows-x64/current");
const forwarded: string[] = [];
for (let index = 0; index < args.length; index++) {
  if (args[index] === "--bundle") {
    const path = args[++index];
    if (!path)
      throw new Error("Provide --bundle <extracted portable directory>");
    installation = resolve(path);
  } else forwarded.push(args[index]!);
}
const node = join(
  installation,
  "runtime/node",
  process.platform === "win32" ? "node.exe" : "node",
);
const launcher = join(installation, "program/core/launcher.js");
for (const path of [node, launcher]) {
  if (!(await stat(path).catch(() => undefined))?.isFile())
    throw new Error(
      "Portable payload is missing; assemble it or select an extracted bundle with --bundle",
    );
}
const located = forwarded.some((arg) => /^--(?:home|locator)(?:=|$)/.test(arg));
const child = spawn(
  node,
  [
    launcher,
    ...(located ? [] : ["--locator", await portableLocator(installation)]),
    ...(forwarded.length ? forwarded : ["serve"]),
  ],
  {
    stdio: "inherit",
    windowsHide: true,
  },
);
child.once("exit", (code) => {
  process.exitCode = code ?? 1;
});
child.once("error", (error) => {
  console.error(error.message);
  process.exitCode = 1;
});

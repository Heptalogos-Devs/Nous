// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { defineCommand } from "citty";
import { commonArgs, stringArgs } from "./command-args.js";
import type { CliLeaf } from "./command-types.js";

import { CliError } from "../agent.js";
import type { CliEnvironment } from "../runtime.js";
import { typedCognition } from "./cognition.js";

async function journalCommon(
  env: CliEnvironment,
  action: string,
  argument?: string,
) {
  const common = await typedCognition.commonCommand(
    env,
    "journal",
    action,
    argument,
    { list: true, history: true },
  );
  if (common.handled) return common.value;
  throw new CliError("INTERNAL_FAILURE", "Journal command is not handled");
}

const journalList = (env: CliEnvironment) => journalCommon(env, "list");
const journalGet = (env: CliEnvironment, argument?: string) =>
  journalCommon(env, "get", argument);
const journalRevision = (env: CliEnvironment, argument?: string) =>
  journalCommon(env, "revision", argument);
const journalHistory = (env: CliEnvironment, argument?: string) =>
  journalCommon(env, "history", argument);
const journalSuppress = (env: CliEnvironment, argument?: string) =>
  journalCommon(env, "suppress", argument);
const journalRestore = (env: CliEnvironment, argument?: string) =>
  journalCommon(env, "restore", argument);
const journalWithdraw = (env: CliEnvironment, argument?: string) =>
  journalCommon(env, "withdraw", argument);
const journalReaccept = (env: CliEnvironment, argument?: string) =>
  journalCommon(env, "reaccept", argument);
const journalPurge = (env: CliEnvironment, argument?: string) =>
  journalCommon(env, "purge", argument);

export function createJournalCommand(leaf: CliLeaf) {
  return defineCommand({
    args: commonArgs,
    subCommands: {
      list: leaf(
        { ...stringArgs("page-size", "page-token", "status") },
        journalList,
      ),
      get: leaf({}, journalGet, { parameters: ["<Journal reference>"] }),
      revision: leaf({}, journalRevision, {
        parameters: ["<exact Journal revision reference>"],
      }),
      history: leaf(
        { ...stringArgs("page-size", "page-token") },
        journalHistory,
        { parameters: ["<Journal reference>"] },
      ),
      suppress: leaf(
        { ...stringArgs("expected-epoch", "operation-id") },
        journalSuppress,
        { parameters: ["<Journal reference>"] },
      ),
      restore: leaf(
        { ...stringArgs("expected-epoch", "operation-id") },
        journalRestore,
        { parameters: ["<Journal reference>"] },
      ),
      withdraw: leaf(
        { ...stringArgs("expected-epoch", "operation-id") },
        journalWithdraw,
        { parameters: ["<Journal reference>"] },
      ),
      reaccept: leaf(
        { ...stringArgs("expected-epoch", "operation-id") },
        journalReaccept,
        { parameters: ["<Journal reference>"] },
      ),
      purge: leaf(
        { ...stringArgs("expected-epoch", "operation-id") },
        journalPurge,
        { parameters: ["<Journal reference>"] },
      ),
    },
  });
}

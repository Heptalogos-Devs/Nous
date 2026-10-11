// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import {
  defineCommand,
  runCommand,
  type ArgsDef,
  type ArgDef,
  type CommandContext,
} from "citty";
import { connectNousInstance } from "@nous-wave/client/node";
import {
  createEnvironment,
  stringFlags,
  booleanFlags,
  listFlags,
  type CliValues,
  type CliEnvironment,
} from "../runtime.js";
import { semanticOutput } from "../output.js";
import { CliError, commandInventory } from "../agent.js";
import { nousqlHelp } from "../nousql-help.js";
import { configCommands } from "./config.js";
import { statusCommands } from "./status.js";
import { subjectCommands } from "./subject.js";
import { sessionCommands } from "./session.js";
import { observeCommands } from "./observe.js";
import { deriveCommands } from "./derive.js";
import { formCommands } from "./form.js";
import { embeddingsCommands } from "./embeddings.js";
import { identityCommands } from "./identity.js";
import { maintenanceCommands } from "./maintenance.js";
import { tagCommands } from "./tag.js";
import { associationCommands } from "./association.js";
import { useCommands } from "./use.js";
import { traceCommands } from "./trace.js";
import { contextCommands } from "./context.js";
import { queryCommands, showCommands, readCommands } from "./query.js";
const argsDef: ArgsDef = Object.fromEntries<ArgDef>([
  ...stringFlags.map((name) => [name, { type: "string" }] as const),
  ...listFlags.map((name) => [name, { type: "string" }] as const),
  ...booleanFlags.map((name) => [name, { type: "boolean" }] as const),
]);
const defaults: CliValues = {
  consumer: "consumer:nous-cli:default",
  "max-operations": "1",
  "max-model-calls": "1",
  "max-elapsed-ms": "30000",
  "page-size": "50",
  "max-nodes": "64",
  "max-depth": "2",
  "max-batches": "16",
};
function valuesOf(ctx: CommandContext): CliValues {
  for (const key of Object.keys(ctx.args))
    if (
      key !== "_" &&
      !Object.keys(argsDef).some(
        (name) =>
          name === key ||
          name.replace(/-([a-z])/g, (_, letter: string) =>
            letter.toUpperCase(),
          ) === key,
      )
    )
      throw new CliError("INVALID_ARGUMENT", `Unknown option --${key}`);
  const values: CliValues = {};
  for (const name of stringFlags)
    if (typeof ctx.args[name] === "string") values[name] = ctx.args[name];
  for (const name of booleanFlags)
    if (typeof ctx.args[name] === "boolean") values[name] = ctx.args[name];
  for (const name of listFlags) {
    const value: unknown = ctx.args[name];
    if (typeof value === "string")
      values[name] = value.split(",").filter(Boolean);
  }
  return values;
}
export async function runCli(
  rawArgs: string[],
  connect = connectNousInstance,
  present?: (value: unknown, env: CliEnvironment) => Promise<unknown>,
  input?: AsyncIterable<string | Uint8Array>,
) {
  let globals: CliValues = defaults;
  let result: unknown;
  let completed = false;
  type Handler = (
    env: CliEnvironment,
    action?: string,
    argument?: string,
    positionals?: string[],
  ) => Promise<unknown>;
  const leaf = (family: string, handler: Handler, action?: string) =>
    defineCommand({
      args: argsDef,
      async run(ctx) {
        const values = { ...globals, ...valuesOf(ctx) };
        const env = await createEnvironment(values, connect, input);
        if (!["config", "status", "subject", "retry"].includes(family))
          env.required(
            env.subjectId,
            "Selected Subject (run subject create/use)",
          );
        result = await handler(
          env,
          action ?? ctx.args._[0],
          action ? ctx.args._[0] : ctx.args._[1],
          [family, ...(action ? [action] : []), ...ctx.args._],
        );
        if (present && !values.raw && !values.json && !values.developer) {
          try {
            result = await present(result, env);
          } catch {
            env.notices.push({
              code: "PRESENTATION_UNAVAILABLE",
              message:
                "Operation result is preserved with canonical references; address presentation is unavailable",
            });
          }
        }
        if (!values.raw)
          result = semanticOutput(
            action ? `${family}.${action}` : family,
            result,
          );
        if (env.notices.length)
          result = values.raw
            ? { data: result, notices: env.notices }
            : { ...(result as object), notices: env.notices };
        completed = true;
      },
    });
  const family = (name: string, handler: Handler, actions: string[]) =>
    defineCommand({
      args: argsDef,
      subCommands: Object.fromEntries(
        actions.map((action) => [action, leaf(name, handler, action)]),
      ),
    });
  const query = family("query", queryCommands, ["prepare", "inspect"]);
  query.setup = (ctx) => {
    if (!["prepare", "inspect"].includes(ctx.args._[0] ?? ""))
      ctx.cmd.subCommands = {};
  };
  query.run = async (ctx) => {
    if (!completed) await leaf("query", queryCommands).run!(ctx);
  };
  const help = defineCommand({
    args: argsDef,
    subCommands: {
      ...Object.fromEntries(
        [
          ...new Set(
            commandInventory.commands.map(
              (command) => command.command.split(" ")[0]!,
            ),
          ),
        ].map((name) => [
          name,
          defineCommand({
            args: argsDef,
            run() {
              result = semanticOutput(
                `help.${name}`,
                commandInventory.commands.filter(
                  (command) => command.command.split(" ")[0] === name,
                ),
              );
              completed = true;
            },
          }),
        ]),
      ),
      nousql: defineCommand({
        args: argsDef,
        run() {
          result = semanticOutput("help.nousql", nousqlHelp);
          completed = true;
        },
      }),
    },
    run() {
      if (!completed) {
        result = semanticOutput("help", commandInventory);
        completed = true;
      }
    },
  });
  const root = defineCommand({
    args: argsDef,
    setup(ctx) {
      globals = { ...defaults, ...valuesOf(ctx) };
      if (globals.raw && !globals.developer)
        throw new CliError("INVALID_ARGUMENT", "--raw requires --developer");
    },
    subCommands: {
      help,
      status: leaf("status", statusCommands),
      config: family("config", configCommands, [
        "list",
        "describe",
        "get",
        "set",
        "clear",
      ]),
      subject: family("subject", subjectCommands, ["create", "list", "use"]),
      session: family("session", sessionCommands, ["open", "show", "close"]),
      observe: family("observe", observeCommands, ["text", "file"]),
      derive: leaf("derive", deriveCommands),
      form: leaf("form", formCommands),
      embeddings: family("embeddings", embeddingsCommands, ["prepare"]),
      identity: family("identity", identityCommands, ["bind", "resolve"]),
      maintenance: family("maintenance", maintenanceCommands, ["grant"]),
      tag: family("tag", tagCommands, [
        "create",
        "get",
        "list",
        "search",
        "resolve",
        "revise",
        "merge",
        "split",
        "attach",
      ]),
      association: family("association", associationCommands, [
        "create",
        "revoke",
        "neighborhood",
      ]),
      query,
      use: leaf("use", useCommands),
      trace: leaf("trace", traceCommands),
      show: leaf("show", (env, ref) => showCommands(env, ref)),
      read: leaf("read", (env, ref) => readCommands(env, ref)),
      context: family("context", contextCommands, [
        "create",
        "list",
        "show",
        "set",
        "pin",
        "unpin",
        "clear",
        "pause",
        "resume",
        "end",
        "select",
        "foreground",
      ]),
      retry: leaf("retry", (env, id) =>
        env.retry(env.required(id, "Receipt ID")),
      ),
    },
    run() {
      return completed ? result : semanticOutput("help", commandInventory);
    },
  });
  return (await runCommand(root, { rawArgs })).result;
}

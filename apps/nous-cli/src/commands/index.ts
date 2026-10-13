// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { resolve } from "node:path";
import type { SystemStatus } from "@nous-wave/protocol/nous/wave/v1alpha1/management_pb.js";
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
import { materialCommands } from "./material.js";
import { memoryCommands } from "./memory.js";
import { schemaCommands } from "./schema.js";
import { episodeCommands, journalCommands } from "./longitudinal.js";
import { resourceCommands } from "./resource.js";
import { projectionCommands } from "./projection.js";
const argsDef: ArgsDef = Object.fromEntries<ArgDef>([
  ...stringFlags.map((name) => [name, { type: "string" }] as const),
  ...listFlags.map((name) => [name, { type: "string" }] as const),
  ...booleanFlags.map((name) => [name, { type: "boolean" }] as const),
]);
const defaults: CliValues = {
  consumer: "default",
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
        let snapshot: SystemStatus | undefined;
        let connection: Awaited<ReturnType<typeof connect>> | undefined;
        if (family === "status") {
          if (!values["run-root"])
            throw new CliError(
              "INVALID_ARGUMENT",
              "--run-root or nous launcher required",
            );
          connection = await connect({ runRoot: resolve(values["run-root"]) });
          snapshot = await connection.system.getStatus({});
          if (
            !snapshot.components.some(
              (component) =>
                component.name === "kernel" && component.state === "READY",
            )
          ) {
            result = values.raw
              ? { status: snapshot }
              : semanticOutput("status", { status: snapshot });
            completed = true;
            return;
          }
        }
        const env = await createEnvironment(
          values,
          connection ? async () => connection! : connect,
          input,
        );
        result =
          family === "status"
            ? await statusCommands(env, snapshot)
            : await handler(
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
      status: leaf("status", (env) => statusCommands(env)),
      config: family("config", configCommands, [
        "list",
        "describe",
        "get",
        "set",
        "clear",
      ]),
      subject: family("subject", subjectCommands, [
        "create",
        "list",
        "use",
        "seed",
        "adopt-seed",
      ]),
      session: family("session", sessionCommands, [
        "open",
        "show",
        "list",
        "close",
      ]),
      observe: family("observe", observeCommands, ["text", "file"]),
      derive: leaf("derive", deriveCommands),
      form: leaf("form", formCommands),
      embeddings: family("embeddings", embeddingsCommands, ["prepare"]),
      identity: family("identity", identityCommands, [
        "bind",
        "resolve",
        "rebind",
      ]),
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
      material: family("material", materialCommands, [
        "clean",
        "limits",
        "artifacts",
        "occurrences",
        "representations",
        "producer",
      ]),
      memory: family("memory", memoryCommands, [
        "form",
        "get",
        "list",
        "revision",
        "history",
        "revise",
        "accessibility",
        "link",
        "suppress",
        "restore",
        "withdraw",
        "reaccept",
        "purge",
      ]),
      schema: family("schema", schemaCommands, [
        "create",
        "get",
        "revision",
        "evidence",
        "revise",
        "split",
        "merge",
        "suppress",
        "restore",
        "withdraw",
        "reaccept",
        "purge",
      ]),
      episode: family("episode", episodeCommands, [
        "create",
        "get",
        "list",
        "revision",
        "history",
        "revise",
        "link",
        "suppress",
        "restore",
        "withdraw",
        "reaccept",
        "purge",
      ]),
      journal: family("journal", journalCommands, [
        "get",
        "list",
        "revision",
        "history",
        "suppress",
        "restore",
        "withdraw",
        "reaccept",
        "purge",
      ]),
      resource: family("resource", resourceCommands, [
        "put",
        "get",
        "list",
        "remove",
        "materialize",
      ]),
      projection: family("projection", projectionCommands, [
        "build",
        "managed",
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
      retry: leaf("retry", (env, id, argument) =>
        id === "forget"
          ? env.forget(env.required(argument, "Saved request ID"))
          : env.retry(env.required(id, "Saved request ID")),
      ),
    },
    run() {
      return completed ? result : semanticOutput("help", commandInventory);
    },
  });
  return (await runCommand(root, { rawArgs })).result;
}

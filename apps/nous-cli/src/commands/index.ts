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
  type CliValues,
  type CliEnvironment,
} from "../runtime.js";
import { semanticOutput } from "../output.js";
import { CliError, commandInventory } from "../agent.js";
import {
  commandDescriptors,
  commandFlags,
  sharedFlags,
  type CommandDescriptor,
} from "./command-descriptors.js";
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
import { memoryCommands } from "./memory.js";
import { resourceCommands } from "./resource.js";
import { schemaCommands } from "./schema.js";
import { episodeCommands } from "./episode.js";
import { journalCommands } from "./journal.js";
import { operationCommands } from "./operation.js";
import { materialCommands } from "./material.js";

type Handler = (
  env: CliEnvironment,
  action?: string,
  argument?: string,
  positionals?: string[],
) => Promise<unknown>;

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
const rootArgs: ArgsDef = Object.fromEntries<ArgDef>(
  sharedFlags.map(({ name, type, description }) => [
    name,
    { type, description },
  ]),
);
const descriptorByPath = new Map(
  commandDescriptors.map((descriptor) => [descriptor.command, descriptor]),
);
const camel = (name: string) =>
  name.replace(/-([a-z])/g, (_, letter: string) => letter.toUpperCase());
const listFlagNames = new Set(["tag", "aboutness", "alias"]);

function descriptorFor(path: string): CommandDescriptor {
  const descriptor = descriptorByPath.get(path);
  if (!descriptor) throw new Error(`Command descriptor missing: ${path}`);
  return descriptor;
}

function argsFor(descriptor: CommandDescriptor): ArgsDef {
  return Object.fromEntries<ArgDef>(
    commandFlags(descriptor).map(({ name, type, description }) => [
      name,
      { type, description },
    ]),
  );
}

function rejectUnknownOptions(
  rawArgs: readonly string[],
  descriptor: CommandDescriptor,
) {
  const allowed = new Set(commandFlags(descriptor).map(({ name }) => name));
  for (const raw of rawArgs) {
    if (!raw.startsWith("--") || raw === "--") continue;
    const name = raw.slice(2).split("=", 1)[0]!;
    if (!allowed.has(name))
      throw new CliError("INVALID_ARGUMENT", `Unknown option --${name}`);
  }
}

function valuesOf(
  ctx: CommandContext,
  descriptor?: CommandDescriptor,
): CliValues {
  const flags = descriptor ? commandFlags(descriptor) : sharedFlags;
  const values: CliValues = {};
  for (const { name, type } of flags) {
    const value: unknown = ctx.args[name] ?? ctx.args[camel(name)];
    if (type === "string" && typeof value === "string")
      if (listFlagNames.has(name))
        values[name as "tag" | "aboutness" | "alias"] = value
          .split(",")
          .filter(Boolean);
      else values[name as keyof CliValues] = value as never;
    if (type === "boolean" && typeof value === "boolean")
      values[name as keyof CliValues] = value as never;
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
  const leaf = (path: string, handler: Handler) => {
    const descriptor = descriptorFor(path);
    return defineCommand({
      args: argsFor(descriptor),
      async run(ctx) {
        rejectUnknownOptions(ctx.rawArgs, descriptor);
        const values = {
          ...globals,
          ...valuesOf(ctx, descriptor),
        };
        const env = await createEnvironment(values, connect, input);
        const parts = path.split(" ");
        const family = parts[0]!;
        const action = parts[1];
        if (
          ![
            "config",
            "status",
            "subject",
            "retry",
            "operation",
            "help",
          ].includes(family) &&
          !(family === "material" && action === "limits")
        )
          env.required(
            env.subjectId,
            "Selected Subject (run subject create/use)",
          );
        const argument = ctx.args._[0];
        result = await handler(env, action, argument, [
          ...parts,
          ...ctx.args._,
        ]);
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
        result = semanticOutput(path.replaceAll(" ", "."), result);
        if (env.notices.length)
          result = { ...(result as object), notices: env.notices };
        completed = true;
      },
    });
  };

  const familyHandlers: Record<string, Handler> = {
    config: configCommands,
    subject: subjectCommands,
    session: sessionCommands,
    observe: observeCommands,
    embeddings: embeddingsCommands,
    identity: identityCommands,
    maintenance: maintenanceCommands,
    tag: tagCommands,
    association: associationCommands,
    memory: memoryCommands,
    resource: resourceCommands,
    schema: schemaCommands,
    episode: episodeCommands,
    journal: journalCommands,
    operation: operationCommands,
    query: queryCommands,
    context: contextCommands,
    material: materialCommands,
  };
  const directHandlers: Record<string, Handler> = {
    status: statusCommands,
    derive: (env, _action, argument) => deriveCommands(env, argument),
    form: (env, _action, argument) => formCommands(env, argument),
    use: (env, _action, argument) => useCommands(env, argument),
    trace: (env, _action, argument) => traceCommands(env, argument),
    show: async (_env, _action, argument) => showCommands(_env, argument),
    read: async (_env, _action, argument) => readCommands(_env, argument),
    retry: async (env, _action, argument) =>
      env.retry(env.required(argument, "Receipt ID")),
  };
  const pathsByRoot = new Map<string, string[]>();
  for (const descriptor of commandDescriptors) {
    const [root, action] = descriptor.command.split(" ");
    if (!root || root === "help" || !action) continue;
    const actions = pathsByRoot.get(root) ?? [];
    actions.push(action);
    pathsByRoot.set(root, actions);
  }

  const help = defineCommand({
    args: rootArgs,
    subCommands: {
      ...Object.fromEntries(
        [
          ...new Set(
            commandDescriptors.map(({ command }) => command.split(" ")[0]!),
          ),
        ]
          .filter((name) => name !== "help")
          .map((name) => [
            name,
            defineCommand({
              args: rootArgs,
              run() {
                result = semanticOutput(
                  `help.${name}`,
                  commandDescriptors.filter(
                    (descriptor) => descriptor.command.split(" ")[0] === name,
                  ),
                );
                completed = true;
              },
            }),
          ]),
      ),
      nousql: defineCommand({
        args: rootArgs,
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

  const subCommands: Record<string, ReturnType<typeof defineCommand>> = {
    help,
  };
  for (const descriptor of commandDescriptors) {
    const [root, action] = descriptor.command.split(" ");
    if (!root || root === "help") continue;
    if (!action) {
      const handler = directHandlers[root];
      if (handler) subCommands[root] = leaf(root, handler);
    }
  }
  for (const [root, actions] of pathsByRoot) {
    const handler = familyHandlers[root];
    if (!handler) throw new Error(`Command handler missing: ${root}`);
    const family = defineCommand({
      args: rootArgs,
      subCommands: Object.fromEntries(
        actions.map((action) => [action, leaf(`${root} ${action}`, handler)]),
      ),
    });
    if (root === "query") {
      family.setup = (ctx) => {
        if (!actions.includes(ctx.args._[0] ?? "")) ctx.cmd.subCommands = {};
      };
      family.run = async (ctx) => {
        if (!completed) await leaf("query", queryCommands).run!(ctx);
      };
    }
    subCommands[root] = family;
  }
  const root = defineCommand({
    args: rootArgs,
    setup(ctx) {
      globals = { ...defaults, ...valuesOf(ctx) };
      if (globals.raw && !globals.developer)
        throw new CliError("INVALID_ARGUMENT", "--raw requires --developer");
      if (!ctx.args._.length)
        rejectUnknownOptions(ctx.rawArgs, { command: "help", flags: [] });
    },
    subCommands,
    run() {
      if (!completed) {
        result = semanticOutput("help", commandInventory);
        completed = true;
      }
    },
  });
  await runCommand(root, { rawArgs });
  return result;
}

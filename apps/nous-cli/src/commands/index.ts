// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import {
  defineCommand,
  runCommand,
  type ArgsDef,
  type ArgDef,
  type CommandContext,
  type CommandDef,
  type CommandMeta,
} from "citty";
import { connectNousInstance } from "@nous-wave/client/node";
import {
  createEnvironment,
  type CliValues,
  type CliEnvironment,
} from "../runtime.js";
import { semanticOutput } from "../output.js";
import { CliError } from "../agent.js";
import { nousqlHelp } from "../nousql-help.js";
import { commonArgs } from "./command-args.js";
import type { CliAction, CliLeaf, CliHelp } from "./command-types.js";
import { createAssociationCommand } from "./association.js";
import { createConfigCommand } from "./config.js";
import { createContextCommand } from "./context.js";
import { createDeriveCommand } from "./derive.js";
import { createEmbeddingsCommand } from "./embeddings.js";
import { createEpisodeCommand } from "./episode.js";
import { createFormCommand } from "./form.js";
import { createIdentityCommand } from "./identity.js";
import { createJournalCommand } from "./journal.js";
import { createMaintenanceCommand } from "./maintenance.js";
import { createMaterialCommand } from "./material.js";
import { createMemoryCommand } from "./memory.js";
import { createObserveCommand } from "./observe.js";
import { createOperationCommand, createRetryCommand } from "./operation.js";
import {
  createQueryCommand,
  createReadCommand,
  createShowCommand,
} from "./query.js";
import { createResourceCommand } from "./resource.js";
import { createSchemaCommand } from "./schema.js";
import { createSessionCommand } from "./session.js";
import { createStatusCommand } from "./status.js";
import { createSubjectCommand } from "./subject.js";
import { createTagCommand } from "./tag.js";
import { createTraceCommand } from "./trace.js";
import { createUseCommand } from "./use.js";

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

const camel = (name: string) =>
  name.replace(/-([a-z])/g, (_, letter: string) => letter.toUpperCase());

function valuesOf(ctx: CommandContext, args: ArgsDef): CliValues {
  const values: CliValues = {};
  for (const [name, definition] of Object.entries(args) as [string, ArgDef][]) {
    const value: unknown = ctx.args[name] ?? ctx.args[camel(name)];
    if (definition.type === "string" && typeof value === "string") {
      const separator = (definition as ArgDef & { separator?: string })
        .separator;
      values[name as keyof CliValues] = (
        separator ? value.split(separator).filter(Boolean) : value
      ) as never;
    }
    if (definition.type === "boolean" && typeof value === "boolean")
      values[name as keyof CliValues] = value as never;
  }
  return values;
}

function rejectUnknownOptions(rawArgs: readonly string[], args: ArgsDef) {
  const allowed = new Set(Object.keys(args));
  for (const raw of rawArgs) {
    if (!raw.startsWith("--") || raw === "--") continue;
    const name = raw.slice(2).split("=", 1)[0]!;
    if (!allowed.has(name))
      throw new CliError("INVALID_ARGUMENT", `Unknown option --${name}`);
  }
}

type CliMeta = CommandMeta & CliHelp;

const helpMeta = {
  name: "help",
  description: "Show command inventory without connecting to Core.",
  parameters: ["[topic]"],
} satisfies CliMeta;

function inventoryFrom(commands: Record<string, CommandDef>) {
  const entries: Record<string, unknown>[] = [];
  const visit = (command: CommandDef) => {
    const meta = command.meta as CliMeta | undefined;
    if (meta?.name) {
      const flags = (Object.entries(command.args ?? {}) as [string, ArgDef][])
        .filter(([name]) => !Object.hasOwn(commonArgs, name))
        .map(([name, definition]) => ({
          name,
          type: definition.type ?? "string",
          ...(definition.description
            ? { description: definition.description }
            : {}),
        }));
      entries.push({
        command: meta.name,
        flags,
        ...(meta.parameters ? { parameters: meta.parameters } : {}),
        ...(meta.description ? { description: meta.description } : {}),
        ...(meta.example ? { example: meta.example } : {}),
      });
    }
    for (const child of Object.values(
      command.subCommands ?? {},
    ) as CommandDef[])
      visit(child);
  };
  for (const command of Object.values(commands)) visit(command);
  return {
    nousql:
      "nous help nousql [--json]; standalone Agent guide without connecting to a daemon",
    contextFlags: {
      subject: "Subject lexical reference overrides local selection",
      session: "Session lexical reference overrides local selection",
      "work-context": "WorkContext lexical reference overrides local selection",
    },
    output: {
      success:
        "nous.cli.v2 envelope; complete command data; protobuf 64-bit integers use decimal strings",
      error:
        "stderr safe error envelope with owner/code/recovery/context/trace and receipt when available; nonzero exit",
    },
    commands: entries,
  };
}

function nameCommands(
  commands: Record<string, CommandDef>,
  prefix: string[] = [],
) {
  for (const [key, command] of Object.entries(commands)) {
    const path = [...prefix, key];
    const meta = (command.meta ?? {}) as CliMeta;
    command.meta = { ...meta, name: path.join(" ") };
    const children = command.subCommands as
      Record<string, CommandDef> | undefined;
    if (children) nameCommands(children, path);
  }
}

function createCommandTrees(directLeaf: CliLeaf): Record<string, CommandDef> {
  return {
    status: createStatusCommand(directLeaf),
    config: createConfigCommand(directLeaf),
    subject: createSubjectCommand(directLeaf),
    session: createSessionCommand(directLeaf),
    observe: createObserveCommand(directLeaf),
    derive: createDeriveCommand(directLeaf),
    form: createFormCommand(directLeaf),
    embeddings: createEmbeddingsCommand(directLeaf),
    identity: createIdentityCommand(directLeaf),
    maintenance: createMaintenanceCommand(directLeaf),
    tag: createTagCommand(directLeaf),
    association: createAssociationCommand(directLeaf),
    query: createQueryCommand(directLeaf),
    use: createUseCommand(directLeaf),
    trace: createTraceCommand(directLeaf),
    show: createShowCommand(directLeaf),
    read: createReadCommand(directLeaf),
    context: createContextCommand(directLeaf),
    retry: createRetryCommand(directLeaf),
    operation: createOperationCommand(directLeaf),
    memory: createMemoryCommand(directLeaf),
    resource: createResourceCommand(directLeaf),
    material: createMaterialCommand(directLeaf),
    schema: createSchemaCommand(directLeaf),
    episode: createEpisodeCommand(directLeaf),
    journal: createJournalCommand(directLeaf),
  };
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

  const makeLeaf = (localArgs: ArgsDef, handler: CliAction, help?: CliHelp) => {
    const args = { ...commonArgs, ...localArgs };
    return defineCommand({
      meta: {
        ...(help?.description ? { description: help.description } : {}),
        ...(help?.parameters ? { parameters: help.parameters } : {}),
        ...(help?.example ? { example: help.example } : {}),
      } satisfies CliMeta,
      args,
      setup(ctx) {
        globals = { ...globals, ...valuesOf(ctx, args) };
      },
      async run(ctx) {
        if (completed) return;
        rejectUnknownOptions(ctx.rawArgs, args);
        const values = { ...globals, ...valuesOf(ctx, args) };
        const env = await createEnvironment(values, connect, input);
        const path = ((ctx.cmd.meta as CliMeta).name ?? "").split(" ");
        const family = path[0]!;
        const action = path[1];
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
        result = await handler(env, ctx.args._[0], [...path, ...ctx.args._]);
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
        result = semanticOutput(path.join("."), result);
        if (env.notices.length)
          result = { ...(result as object), notices: env.notices };
        completed = true;
      },
    });
  };
  const directLeaf: CliLeaf = (args, handler, help) =>
    makeLeaf(args, handler, help);

  const commandTrees: Record<string, CommandDef> = {
    help: defineCommand({
      meta: helpMeta,
      args: commonArgs,
      run(ctx) {
        rejectUnknownOptions(ctx.rawArgs, commonArgs);
        const topic = ctx.args._[0];
        if (topic === "nousql") {
          result = semanticOutput("help.nousql", nousqlHelp);
        } else {
          if (topic) {
            const commands = commandInventory.commands.filter(
              (entry) =>
                typeof entry.command === "string" &&
                entry.command.split(" ")[0] === topic,
            );
            result = semanticOutput(`help.${topic}`, commands);
          } else result = semanticOutput("help", commandInventory);
        }
        completed = true;
      },
    }),
    ...createCommandTrees(directLeaf),
  };
  nameCommands(commandTrees);
  const commandInventory = inventoryFrom(commandTrees);

  const root = defineCommand({
    args: commonArgs,
    setup(ctx) {
      globals = { ...defaults, ...valuesOf(ctx, commonArgs) };
      if (globals.raw && !globals.developer)
        throw new CliError("INVALID_ARGUMENT", "--raw requires --developer");
      if (!ctx.args._.length) rejectUnknownOptions(ctx.rawArgs, commonArgs);
    },
    subCommands: commandTrees,
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

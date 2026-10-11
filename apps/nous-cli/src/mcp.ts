// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { McpServer, type CallToolResult } from "@modelcontextprotocol/server";
import { z } from "zod";
import { runCli } from "./commands/index.js";
import { friendlyOutput } from "./friendly.js";
import { formatError, formatResult } from "./format.js";

type Execute = (args: string[]) => Promise<CallToolResult>;

/** Each command freezes its selection; shared state owns short transactions. */
export function createMcpServer(execute: Execute) {
  const server = new McpServer({ name: "nous", version: "0.1.0" });
  const invoke = (args: string[]) => {
    if (
      args[0] === "mcp" ||
      args.some((arg) =>
        /^--(?:run-root|instance-root|state-root|consumer)(?:=|$)/.test(arg),
      )
    )
      return Promise.resolve<CallToolResult>({
        content: [
          {
            type: "text",
            text: "MCP connection owns run-root, state-root and consumer; use --subject/--session for cognition selection.",
          },
        ],
        isError: true,
      });
    return execute(args);
  };
  server.registerTool(
    "nous_help",
    {
      description:
        "Read Nous CLI help without a daemon. Start here, then nous_help topic=nousql. All output is the native CLI text.",
      inputSchema: z.object({ topic: z.string().max(64).optional() }),
    },
    ({ topic }) => invoke(["help", ...(topic ? [topic] : [])]),
  );
  server.registerTool(
    "nous_command",
    {
      description:
        "Execute native Nous CLI argv (no shell). Discover commands with nous_help. This connection has isolated selection, exact result:N references and retry receipts. Default semantic text; use --json only when needed. Failed commands preserve the native error and retry receipt. Subject/Session selection is explicit or durable locally.",
      inputSchema: z.object({
        args: z.array(z.string().max(65536)).min(1).max(128),
      }),
    },
    ({ args }) => invoke(args),
  );
  server.registerTool(
    "nous_query",
    {
      description:
        "Query Nous with Unicode intent and optional NousQL syntax islands. Uses selected Subject/Session/foreground WorkContext and saves exact result:N for show, trace, pin and use. Read nous_help topic=nousql for syntax.",
      inputSchema: z.object({
        query: z.string().min(1).max(65536),
        subject: z.string().optional(),
        session: z.string().optional(),
      }),
    },
    ({ query, subject, session }) =>
      invoke([
        "query",
        query,
        ...(subject ? ["--subject", subject] : []),
        ...(session ? ["--session", session] : []),
      ]),
  );
  return server;
}

/** MCP and Terminal use one command core; only the host owns stdout and stdin. */
export function cliExecutor(options: {
  runRoot: string;
  stateRoot: string;
  consumer: string;
}): Execute {
  return async (args) => {
    const command = [
      "--run-root",
      options.runRoot,
      "--instance-root",
      options.stateRoot,
      "--consumer",
      options.consumer,
      ...args,
    ];
    try {
      const result = await runCli(command, undefined, friendlyOutput);
      return {
        content: [{ type: "text", text: formatResult(result, command) }],
      };
    } catch (error) {
      return {
        content: [
          {
            type: "text",
            text: formatError(error, command),
          },
        ],
        isError: true,
      };
    }
  };
}

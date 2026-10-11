// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { expect, test } from "vitest";
import { createMcpServer } from "../src/mcp.js";
import { Client } from "@modelcontextprotocol/client";
import { InMemoryTransport } from "@modelcontextprotocol/client";
import { StdioClientTransport } from "@modelcontextprotocol/client/stdio";
import { mkdtemp, rm } from "node:fs/promises";
import { join } from "node:path";
import { repositoryRoot, workspacePaths } from "../../../scripts/workspace.js";

test("MCP forwards exact argv without serializing long commands and preserves CLI errors", async () => {
  const seen: string[][] = [];
  let active = 0;
  let peak = 0;
  const server = createMcpServer(async (args) => {
    seen.push(args);
    peak = Math.max(peak, ++active);
    await new Promise((done) => setTimeout(done, 10));
    active--;
    return {
      content: [{ type: "text", text: args.join("\n") }],
      isError: args[0] === "bad",
    };
  });
  const client = new Client({ name: "dogfooding-regression", version: "1" });
  const [a, b] = InMemoryTransport.createLinkedPair();
  await server.connect(a);
  await client.connect(b);
  try {
    expect((await client.listTools()).tools.map((t) => t.name)).toEqual([
      "nous_help",
      "nous_command",
      "nous_query",
    ]);
    const query = "Nous MCP $return(memory) $limit(5); $(echo secret)";
    await Promise.all([
      client.callTool({ name: "nous_query", arguments: { query } }),
      client.callTool({
        name: "nous_command",
        arguments: { args: ["context", "set", "--text", "a & b"] },
      }),
    ]);
    expect(seen).toContainEqual(["query", query]);
    expect(seen).toContainEqual(["context", "set", "--text", "a & b"]);
    expect(peak).toBe(2);
    expect(
      (
        await client.callTool({
          name: "nous_command",
          arguments: { args: ["bad"] },
        })
      ).isError,
    ).toBe(true);
    expect(
      (
        await client.callTool({
          name: "nous_command",
          arguments: { args: ["query", "x", "--instance-root=other"] },
        })
      ).isError,
    ).toBe(true);
    expect(seen).not.toContainEqual(["query", "x", "--instance-root=other"]);
  } finally {
    await client.close();
    await server.close();
  }
});

test("real stdio carries command help and errors as pure MCP messages", async () => {
  const state = await mkdtemp(join(workspacePaths.temporary, "mcp-stdio-"));
  const transport = new StdioClientTransport({
    command: process.execPath,
    args: [
      join(repositoryRoot, "node_modules/tsx/dist/cli.mjs"),
      join(repositoryRoot, "apps/nous-cli/src/mcp-main.ts"),
      "--run-root",
      state,
      "--state-root",
      state,
      "--consumer",
      "consumer:stdio:regression",
    ],
    cwd: repositoryRoot,
    stderr: "pipe",
  });
  let errors = "";
  transport.stderr?.on("data", (data: Buffer) => {
    errors += data.toString();
  });
  const client = new Client({ name: "stdio-regression", version: "1" });
  try {
    await client.connect(transport);
    const help = await client.callTool({
      name: "nous_help",
      arguments: { topic: "nousql" },
    });
    expect(help.isError).not.toBe(true);
    expect(JSON.stringify(help)).toContain("NousQL");
    const json = await client.callTool({
      name: "nous_command",
      arguments: { args: ["help", "--json"] },
    });
    const content = json.content as { text: string }[];
    expect(JSON.parse(content[0]!.text)).toMatchObject({ kind: "help" });
    const failure = await client.callTool({
      name: "nous_command",
      arguments: { args: ["missing-command"] },
    });
    expect(failure.isError).toBe(true);
    expect(JSON.stringify(failure)).toContain("Unknown command");
    expect(errors).toBe("");
  } finally {
    await client.close();
    await transport.close();
    await rm(state, { recursive: true, force: true });
  }
}, 10000);

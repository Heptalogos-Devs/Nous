// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { createServer } from "node:http";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { once } from "node:events";
import { expect, it } from "vitest";
import { startResearchGateway } from "../../research/gateway.js";

it("counts real forwarded attempts including failure and preserves the run cap across proxy restart", async () => {
  const root = await mkdtemp(join(tmpdir(), "nous-research-wire-"));
  const seen: { path: string; body: string; credential: string | undefined }[] =
    [];
  const upstream = createServer((request, response) => {
    void (async () => {
      const bytes: Buffer[] = [];
      for await (const value of request) {
        const chunk: unknown = value;
        if (!(chunk instanceof Uint8Array)) throw new Error("request bytes");
        bytes.push(Buffer.from(chunk));
      }
      seen.push({
        path: request.url!,
        body: Buffer.concat(bytes).toString(),
        credential: request.headers.authorization,
      });
      response.writeHead(seen.length === 2 ? 503 : 200, {
        "Content-Type": "application/json",
      });
      response.end(
        '{"private_text":"fixture-only","usage":{"prompt_tokens":3,"completion_tokens":2,"total_tokens":5,"credential":"fixture-only"}}',
      );
    })().catch(() => response.destroy());
  });
  upstream.listen(0, "127.0.0.1");
  await once(upstream, "listening");
  const address = upstream.address();
  if (!address || typeof address === "string")
    throw new Error("fixture address");
  const options = {
    upstream: `http://127.0.0.1:${address.port}/v1`,
    port: 0,
    ledger: join(root, "calls.json"),
    maxCalls: 3,
    traceRoot: join(root, "traces"),
  };
  let proxy = await startResearchGateway(options);
  const invoke = (path: string) =>
    fetch(proxy.endpoint + path, {
      method: "POST",
      headers: {
        Authorization: "Bearer fixture-only",
        "Content-Type": "application/json",
      },
      body: '{"model":"fixture"}',
    });
  try {
    expect((await invoke("/chat/completions")).status).toBe(200);
    expect((await invoke("/embeddings")).status).toBe(503);
    expect((await invoke("/../admin")).status).toBe(404);
    await proxy.close();
    proxy = await startResearchGateway(options);
    expect((await invoke("/embeddings")).status).toBe(200);
    expect((await invoke("/rerank")).status).toBe(429);
    expect(seen).toHaveLength(3);
    expect(seen[0]).toEqual({
      path: "/v1/chat/completions",
      body: '{"model":"fixture"}',
      credential: "Bearer fixture-only",
    });
    expect(JSON.parse(await readFile(options.ledger, "utf8"))).toEqual({
      count: 3,
    });
    await proxy.close();
    const records = (
      await readFile(options.ledger + ".telemetry.jsonl", "utf8")
    )
      .trim()
      .split("\n")
      .map(
        (line) =>
          JSON.parse(line) as {
            attempt: number;
            latencyMs: number;
            usage: unknown;
            status: number;
          },
      );
    expect(records.map((row) => row.attempt)).toEqual([1, 2, 3]);
    expect(records.map((row) => row.status)).toEqual([200, 503, 200]);
    expect(records.every((row) => row.latencyMs >= 0)).toBe(true);
    expect(records[0]?.usage).toEqual({
      prompt_tokens: 3,
      completion_tokens: 2,
      total_tokens: 5,
    });
    expect(JSON.stringify(records)).not.toContain("fixture-only");
    const trace = await readFile(
      join(options.traceRoot, "1/response.json"),
      "utf8",
    );
    expect(trace).toContain('"total_tokens": 5');
    expect(trace).not.toContain("fixture-only");
    expect(trace).not.toContain('"credential"');
    expect(
      JSON.parse(
        await readFile(join(options.traceRoot, "1/request.json"), "utf8"),
      ),
    ).toEqual({ model: "fixture" });
    const meta = JSON.parse(
      await readFile(join(options.traceRoot, "2/meta.json"), "utf8"),
    ) as { status: number; response_complete: boolean };
    expect(meta.status).toBe(503);
    expect(meta.response_complete).toBe(true);
    proxy = await startResearchGateway(options);
  } finally {
    await proxy.close();
    upstream.closeAllConnections();
    await new Promise<void>((done) => upstream.close(() => done()));
    await rm(root, { recursive: true, force: true });
  }
});

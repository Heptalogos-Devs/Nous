// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { createServer } from "node:http";
import { expect, test } from "vitest";
import { modelConfigurationSchema } from "../src/model/configuration.js";
import { ModelInvocations } from "../src/model/invocations.js";
import { failedExecutionTelemetry } from "../src/model/execution/routes.js";

test("cancellation preserves the failed route and the transmitted attempt whose outcome is unknown", async () => {
  let reached: () => void = () => {};
  const transmitted = new Promise<void>((resolve) => {
    reached = resolve;
  });
  const server = createServer((request, response) => {
    void (async () => {
      const chunks: Uint8Array[] = [];
      for await (const chunk of request) chunks.push(chunk as Uint8Array);
      const { model } = JSON.parse(Buffer.concat(chunks).toString()) as {
        model: string;
      };
      if (model === "first")
        response
          .writeHead(503, { "content-type": "application/json" })
          .end('{"error":"first route refused"}');
      else reached();
    })().catch(() => response.destroy());
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const address = server.address();
  if (!address || typeof address === "string")
    throw new Error("Missing provider port");
  process.env.NOUS_CANCEL_ROUTE = "test-only";
  const configuration = modelConfigurationSchema.parse({
    gateway_profiles: {
      local: {
        base_url: `http://127.0.0.1:${address.port}/v1`,
        credential_env: "NOUS_CANCEL_ROUTE",
      },
    },
    model_profiles: Object.fromEntries(
      ["first", "second"].map((model) => [
        model,
        {
          gateway: "local",
          protocol: "openai-chat",
          model,
          capabilities: ["text", "structured_output"],
        },
      ]),
    ),
    execution_profiles: {
      first: { model: "first" },
      second: { model: "second" },
    },
    roles: { memory_formation: { routes: ["first", "second"] } },
  });
  const cancellation = new AbortController();
  try {
    const models = await ModelInvocations.create(configuration);
    const running = models
      .generate(
        "memory_formation",
        { content: "bounded source" },
        { signal: cancellation.signal },
      )
      .catch((error: unknown) => error);
    await transmitted;
    cancellation.abort(new Error("caller cancelled the bounded operation"));
    const error = await running;
    expect(
      failedExecutionTelemetry(error)?.attempts.map(
        (attempt) => attempt.status,
      ),
    ).toEqual(["failed", "unknown"]);
    expect(failedExecutionTelemetry(error)?.attempts[1]?.failureClass).toBe(
      "caller_cancelled",
    );
    expect(failedExecutionTelemetry(error)?.attempts[1]?.usage).toBeUndefined();
  } finally {
    delete process.env.NOUS_CANCEL_ROUTE;
    server.closeAllConnections();
    await new Promise<void>((resolve) => server.close(() => resolve()));
  }
});

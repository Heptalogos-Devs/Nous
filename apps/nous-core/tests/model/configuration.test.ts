// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { expect, it } from "vitest";
import { modelConfigurationSchema } from "../../src/model/configuration.js";
import { ModelInvocations } from "../../src/model/invocations.js";

const modelGraph = () => ({
  gateway_profiles: {
    local: {
      base_url: "http://127.0.0.1:1/v1",
      credential_env: "NOUS_GRAPH_TEST",
    },
  },
  model_profiles: {
    chat: {
      gateway: "local",
      protocol: "openai-chat",
      model: "configured",
      capabilities: ["text", "structured_output"],
    },
  },
  execution_profiles: { main: { model: "chat" } },
  roles: { memory_formation: { routes: ["main"] } },
});

it("rejects a dangling explicit route with its exact configuration path", () => {
  const config = modelGraph();
  config.roles.memory_formation.routes = ["missing"];
  const parsed = modelConfigurationSchema.safeParse(config);
  expect(parsed.success).toBe(false);
  if (!parsed.success)
    expect(parsed.error.issues[0]?.path.join(".")).toBe(
      "roles.memory_formation.routes.0",
    );
});

it("freezes only a role's dependency graph so unrelated profiles and media budgets do not change identity", async () => {
  process.env.NOUS_GRAPH_TEST = "controlled-owner-input";
  try {
    const first = modelConfigurationSchema.parse(modelGraph());
    const second = modelConfigurationSchema.parse({
      ...modelGraph(),
      video: { max_frames: 2 },
      model_profiles: {
        ...modelGraph().model_profiles,
        dormant: {
          gateway: "local",
          protocol: "openai-chat",
          model: "unused",
          capabilities: ["text"],
        },
      },
    });
    const left = (await ModelInvocations.create(first)).prepare(
      "memory_formation",
    );
    const right = (await ModelInvocations.create(second)).prepare(
      "memory_formation",
    );
    expect(left.configDigest).toBe(right.configDigest);
  } finally {
    delete process.env.NOUS_GRAPH_TEST;
  }
});

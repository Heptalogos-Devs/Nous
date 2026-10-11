// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { createConnectTransport } from "@connectrpc/connect-node";
import { createNousClient } from "./index.js";
import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { artifactUploads } from "./artifacts.js";

export function connectNous(baseUrl: string, token: string) {
  const client = createNousClient(
    createConnectTransport({
      baseUrl,
      httpVersion: "1.1",
      defaultTimeoutMs: 30_000,
      interceptors: [
        (next) => async (request) => {
          request.header.set("authorization", `Bearer ${token}`);
          return next(request);
        },
      ],
    }),
  );
  return {
    ...client,
    artifacts: artifactUploads(baseUrl, token, (options) =>
      client.material.limits({}, options),
    ),
  };
}
export async function connectNousInstance(instance: { runRoot: string }) {
  const value = JSON.parse(
    await readFile(join(instance.runRoot, "core.json"), "utf8"),
  ) as { endpoint?: unknown; token?: unknown; instanceId?: unknown };
  if (
    typeof value.endpoint !== "string" ||
    typeof value.token !== "string" ||
    typeof value.instanceId !== "string" ||
    !/^[0-9a-f-]{36}$/i.test(value.instanceId)
  )
    throw new Error("Invalid Core discovery record");
  const url = new URL(value.endpoint);
  if (url.hostname !== "127.0.0.1" || url.protocol !== "http:")
    throw new Error("Local discovery must use loopback HTTP");
  return {
    ...connectNous(value.endpoint, value.token),
    instanceId: value.instanceId,
  };
}

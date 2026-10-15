// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { serviceDescriptors } from "@nous-wave/client";
import type { connectNousInstance } from "@nous-wave/client/node";
import { CliError } from "./agent.js";

type Connection = Awaited<ReturnType<typeof connectNousInstance>>;
/** Connection scope is set by the operator at startup, never by conversation content. */
export function mcpConnection(
  connection: Connection,
  management: boolean,
): Connection {
  if (management) return connection;
  const scoped = { ...connection };
  for (const [domain, service] of Object.entries(serviceDescriptors)) {
    const source = connection[
      domain as keyof typeof serviceDescriptors
    ] as unknown as Record<string, (...args: unknown[]) => unknown>;
    const methods = { ...source };
    for (const method of service.methods) {
      const name = method.localName;
      const allowed =
        domain === "runtime" ||
        (domain === "resources" && name === "materializeResource") ||
        (domain === "cognition" && name !== "grantMaintenance") ||
        (domain === "model" &&
          ["deriveMaterial", "prepareEmbeddings"].includes(name)) ||
        /^(get|list|read|search|resolve|trace)/.test(name);
      if (!allowed)
        methods[name] = () => {
          throw new CliError(
            "PERMISSION_DENIED",
            `MCP cognition scope does not grant ${domain}.${name}; an operator must start a separate --management connection for authority changes.`,
            [],
            [],
            undefined,
            { commit_status: "rejected" },
          );
        };
    }
    Object.assign(scoped, { [domain]: methods });
  }
  return scoped;
}

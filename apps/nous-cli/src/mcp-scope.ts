// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { serviceDescriptors } from "@nous-wave/client";
import type { connectNousInstance } from "@nous-wave/client/node";
import { create, type MessageInitShape } from "@bufbuild/protobuf";
import { CognitionUsageSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { CliError } from "./agent.js";

type Connection = Awaited<ReturnType<typeof connectNousInstance>>;
/** Only the operator/actual Host supplies startup scope; tool arguments never grant sources. */
export function mcpConnection(
  connection: Connection,
  management: boolean,
  trustedUsage?: MessageInitShape<typeof CognitionUsageSchema>,
): Connection {
  if (management) return connection;
  const scoped = { ...connection };
  Object.defineProperty(scoped, Symbol.for("nous.scopedSourceReader"), {
    value: true,
  });
  const reject = (operation: string): never => {
    throw new CliError(
      "PERMISSION_DENIED",
      `MCP scoped cognition does not grant ${operation}. Use source-scoped query/show for cognition; an operator owns startup SourceRead and separate --management access.`,
      [],
      [],
      undefined,
      { commit_status: "rejected" },
    );
  };
  const usage = (subject: unknown) => {
    const captured = trustedUsage
      ? create(CognitionUsageSchema, trustedUsage)
      : undefined;
    if (!captured?.sourceRead || captured.sourceRead.subjectId !== subject)
      return reject("missing same-Subject trusted startup source scope");
    if (
      !captured.sourceRead.sourceRefs.length &&
      !captured.sourceRead.originConversationRefs.length
    )
      return reject("empty trusted startup source scope");
    return captured;
  };
  for (const [domain, service] of Object.entries(serviceDescriptors)) {
    const source = connection[
      domain as keyof typeof serviceDescriptors
    ] as unknown as Record<string, (...args: unknown[]) => unknown>;
    const methods = { ...source };
    for (const method of service.methods) {
      const name = method.localName;
      if (domain === "system" && name === "getStatus")
        methods[name] = source[name]!;
      else if (
        domain === "cognition" &&
        ["query", "prepareQuery", "readSources"].includes(name)
      )
        methods[name] = (...args) => {
          const input = args[0] as { subjectId: string };
          return source[name]!(
            { ...input, usage: usage(input.subjectId) },
            ...args.slice(1),
          );
        };
      else if (domain === "cognition" && name === "buildManagedContext")
        methods[name] = (...args) => {
          const input = args[0] as { projection?: { subjectId: string } };
          if (!input.projection) return reject(name);
          return source[name]!(
            {
              ...input,
              projection: {
                ...input.projection,
                usage: usage(input.projection.subjectId),
              },
            },
            ...args.slice(1),
          );
        };
      else methods[name] = () => reject(`${domain}.${name}`);
    }
    Object.assign(scoped, { [domain]: methods });
  }
  return scoped;
}

// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { toJson, type Message } from "@bufbuild/protobuf";
import { protocolMessageSchema } from "@nous-wave/client/data";

function protoJson(value: unknown): unknown {
  if (
    value === null ||
    typeof value !== "object" ||
    value instanceof Uint8Array
  )
    return value;
  const schema = protocolMessageSchema(value);
  if (schema) return toJson(schema, value as Message);
  if (Array.isArray(value)) return value.map(protoJson);
  return Object.fromEntries(
    Object.entries(value).map(([key, member]) => [key, protoJson(member)]),
  );
}

/** A full-result CLI envelope; command output never projects away response fields. */
export function semanticOutput(kind: string, input: unknown) {
  return {
    schemaVersion: "nous.cli.v2",
    kind,
    data: protoJson(input),
  };
}

export function renderText(value: unknown, depth = 0): string {
  if (value === undefined || value === null || value === "") return "";
  if (typeof value === "string") return value;
  if (typeof value === "number" || typeof value === "boolean")
    return value.toString();
  if (typeof value !== "object") return "";
  if (Array.isArray(value))
    return value
      .flatMap((item, index) => {
        const body = renderText(item, depth + 1);
        return body
          ? [`${"  ".repeat(depth)}${index + 1}. ${body.trimStart()}`]
          : [];
      })
      .join("\n");
  if ("schemaVersion" in value && "data" in value)
    return renderText(value.data, depth);
  if (
    "kind" in value &&
    "value" in value &&
    typeof value.kind === "string" &&
    typeof value.value === "string"
  ) {
    const displayReferences =
      "displayReferences" in value &&
      value.displayReferences &&
      typeof value.displayReferences === "object"
        ? (value.displayReferences as Record<string, unknown>)
        : undefined;
    const displayed = displayReferences?.value;
    if (typeof displayed === "string") return displayed;
    return value.value.includes(":")
      ? value.value
      : `${value.kind}:${value.value}`;
  }
  if ("case" in value && "value" in value && typeof value.case === "string") {
    const body = renderText(value.value, depth + 1);
    if (!body) return "";
    return body.includes("\n")
      ? `${"  ".repeat(depth)}${value.case}:\n${body}`
      : `${"  ".repeat(depth)}${value.case}: ${body.trimStart()}`;
  }
  const record = value as Record<string, unknown>;
  const displayReferences =
    record.displayReferences && typeof record.displayReferences === "object"
      ? (record.displayReferences as Record<string, unknown>)
      : undefined;
  return Object.entries(record)
    .filter(([key]) => key !== "displayReferences")
    .filter(
      ([, item]) =>
        item !== undefined &&
        item !== "" &&
        (!Array.isArray(item) || item.length),
    )
    .map(([key, item]) => {
      const displayed = displayReferences?.[key];
      const body = renderText(
        typeof displayed === "string"
          ? displayed
          : Array.isArray(displayed) && Array.isArray(item)
            ? (item as unknown[]).map(
                (member, index) => (displayed as unknown[])[index] ?? member,
              )
            : item,
        depth + 1,
      );
      if (!body) return "";
      return body.includes("\n") || Array.isArray(item)
        ? `${"  ".repeat(depth)}${key}:\n${body}`
        : `${"  ".repeat(depth)}${key}: ${body.trimStart()}`;
    })
    .filter(Boolean)
    .join("\n");
}

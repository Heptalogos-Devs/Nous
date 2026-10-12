// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
export function semanticOutput(kind: string, data: unknown) {
  return { schemaVersion: "nous.cli.v1", kind, data };
}

export function renderText(value: unknown, depth = 0): string {
  if (value === undefined || value === null || value === "") return "";
  if (typeof value === "string") return value;
  if (
    typeof value === "number" ||
    typeof value === "boolean" ||
    typeof value === "bigint"
  )
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
  if ("schemaVersion" in value && "data" in value) {
    const body = renderText(value.data, depth);
    const notices =
      "notices" in value ? renderText({ notices: value.notices }, depth) : "";
    return [body, notices].filter(Boolean).join("\n");
  }
  return Object.entries(value)
    .filter(
      ([_key, item]) =>
        item !== undefined &&
        item !== "" &&
        (!Array.isArray(item) || item.length),
    )
    .map(([key, item]) => {
      const body = renderText(item, depth + 1);
      if (!body) return "";
      return body.includes("\n") || Array.isArray(item)
        ? `${"  ".repeat(depth)}${key}:\n${body}`
        : `${"  ".repeat(depth)}${key}: ${body.trimStart()}`;
    })
    .filter(Boolean)
    .join("\n");
}

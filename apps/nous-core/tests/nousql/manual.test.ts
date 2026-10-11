// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { readFile } from "node:fs/promises";
import { expect, it } from "vitest";
import { fromJson } from "@bufbuild/protobuf";
import { TimestampSchema } from "@bufbuild/protobuf/wkt";
import { parse } from "../../src/nousql/parser.js";
import { compileNousQL } from "../../src/nousql/compiler.js";
import { nousqlHelp } from "../../../nous-cli/src/nousql-help.js";

it("keeps all Agent manual and structured help queries executable", async () => {
  const manual = (
    await readFile(
      new URL("../../../../docs/agent/NOUSQL.md", import.meta.url),
      "utf8",
    )
  ).replace(/\r\n/g, "\n");
  const examples = [...manual.matchAll(/```nousql\n([\s\S]*?)\n```/g)].map(
    (match) => match[1]!,
  );
  expect(examples.length).toBeGreaterThanOrEqual(18);
  for (const example of [...examples, ...nousqlHelp.examples]) {
    expect(() => parse(example), example).not.toThrow();
    const compiled = await compileNousQL(
      example,
      async (kind, locator) => ({
        canonical: {
          kind: kind || "memory",
          value: `fixture:${kind}:${locator.value}`,
        },
        lexicalRef:
          locator.kind === "lexical" ? locator.value : "tag:kavaj-logiv-bufog",
      }),
      fromJson(TimestampSchema, "2026-10-07T00:00:00Z"),
    );
    expect(compiled.expression, example).toBeDefined();
  }
});

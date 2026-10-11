// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { expect, it } from "vitest";
import { fromJson } from "@bufbuild/protobuf";
import { TimestampSchema } from "@bufbuild/protobuf/wkt";
import { compileNousQL } from "../src/nousql/compiler.js";
import { canonical, parse } from "../src/nousql/parser.js";
const now = fromJson(TimestampSchema, "2026-10-07T00:00:00Z");
const resolve = async () => ({
  canonical: { kind: "tag", value: "tag-id" },
  lexicalRef: "tag:kavaj-logiv-bufog",
});
it("separates root projection from query intent and rejects obsolete directives", async () => {
  const result = await compileNousQL(
    "migration $return(schema,memory)",
    resolve,
    now,
  );
  expect(result.expression.modifiers?.projection?.domains).toEqual([
    "memory",
    "schema",
  ]);
  expect(result.boundCanonical).toContain("$return(memory,schema)");
  for (const domain of [
    "memory",
    "schema",
    "episode",
    "journal",
    "evidence",
    "resource",
  ]) {
    await expect(
      compileNousQL(`migration $${domain}`, resolve, now),
    ).rejects.toThrow("Unknown directive");
  }
  for (const text of [
    "migration $return(memory,memory)",
    "migration $return(cognition,schema)",
    '("a" $return(memory)) || "b"',
    "a $return(memory) $return(schema)",
  ]) {
    await expect(compileNousQL(text, resolve, now)).rejects.toThrow();
  }
});
it("captures relative as-of and multiple independent time axes", async () => {
  const result = await compileNousQL(
    'policy $asof(ago=30d) $history $time(observed,within=7d) $time(occurred,from="2026-01-01T00:00:00Z")',
    resolve,
    now,
  );
  const m = result.expression.modifiers!;
  expect(m.asOf?.seconds).toBe(now.seconds - 30n * 86400n);
  expect(m.history).toBe(true);
  expect(m.clockNow?.seconds).toBe(now.seconds);
  const observed = m.constraints?.observed?.predicate;
  expect(observed?.case === "range" && observed.value.end?.seconds).toBe(
    m.clockNow?.seconds,
  );
  const occurred = m.constraints?.occurred?.predicate;
  expect(occurred?.case === "range" && occurred.value.start?.seconds).toBe(
    1767225600n,
  );
  expect(result.boundCanonical).toContain('$asof("2026-09-07T00:00:00Z")');
  expect(result.boundCanonical).not.toContain("within=");
  await expect(
    compileNousQL(
      "a $time(observed,within=7d) $time(observed,within=30d)",
      resolve,
      now,
    ),
  ).rejects.toThrow("Duplicate");
  for (const text of [
    'a $asof("2026-01-01")',
    "a $asof(ago=0d)",
    'a $asof("2026-01-01T00:00:00Z",ago=30d)',
    '("a" $history) || "b"',
    '("a" $asof(ago=30d)) || "b"',
    'a $time(valid,from="2026-01-01T00:00:00Z",to="2026-01-01T00:00:00Z")',
  ]) {
    await expect(compileNousQL(text, resolve, now)).rejects.toThrow();
  }
  const clock = fromJson(TimestampSchema, "2026-10-07T00:00:00.123456789Z");
  const precise = await compileNousQL(
    'policy $asof(ago=1ms) $time(formed,at="2026-10-07T09:00:00.123456789+09:00") $time(observed,within=1ms)',
    resolve,
    clock,
  );
  const modifiers = precise.expression.modifiers!;
  const point = modifiers.constraints?.formed?.predicate;
  expect(point?.case).toBe("point");
  expect(point?.case === "point" && point.value.nanos).toBe(123456789);
  expect(modifiers.clockNow).toEqual(clock);
  expect(modifiers.asOf?.nanos).toBe(122456789);
  const range = modifiers.constraints?.observed?.predicate;
  expect(range?.case === "range" && range.value.start?.nanos).toBe(122456789);
  expect(precise.boundCanonical).toContain(
    'at="2026-10-07T00:00:00.123456789Z"',
  );
});
it("keeps semantic concept phrases distinct from durable Tag selectors", () => {
  const text = "Recall reclamation #active-reader-reclamation";
  expect(canonical(parse(text))).toBe(text);
  expect(parse(text).cues[0]?.kind).toBe("concept");
  expect(
    parse('Recall reclamation @tag("active-reader reclamation")').cues[0]?.kind,
  ).toBe("selector");
});

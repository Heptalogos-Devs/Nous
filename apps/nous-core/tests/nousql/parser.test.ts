// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { expect, it } from "vitest";
import { fromJson, toJson } from "@bufbuild/protobuf";
import { TimestampSchema, type Timestamp } from "@bufbuild/protobuf/wkt";
import { canonical, parse } from "../../src/nousql/parser.js";
import { compileNousQL } from "../../src/nousql/compiler.js";
const now = fromJson(TimestampSchema, "2026-09-17T00:00:00Z");
const resolve = async (kind: string, locator: { value: string }) => ({
  canonical: { kind, value: `entity:${locator.value}` },
  lexicalRef:
    locator.value === "Alice"
      ? "ent:fasid-jizih-kudah"
      : "ent:rofas-nagal-lifod",
});
it("accepts Unicode intent, pronouns and ordinary punctuation without closure heuristics", () => {
  for (const text of [
    "我和他在这个项目进展上如何",
    "How did we progress on that project?",
    "C# email alice@example.org + costs - taxes && revenue || expenses",
    "λ calculus; 数据库 / deployment?",
    "彼は後で何を変更しましたか？",
    "كيف تغيّر المشروع لاحقًا؟",
    "Remember 🚀 release 🧠 notes",
    String.raw`Recall C:\Users\Alice\project notes`,
    "What did @mention decide?",
  ])
    expect(parse(text).intentText).toBe(text);
});
it("distinguishes escaped literal sigils from typed islands and round-trips canonically", () => {
  const syntax = parse(
    String.raw`Find \$PATH \@tag(plain) \#literal \\folder #迁移 #("reader reclamation") $prefer("rollback") $avoid(recent,observed)`,
  );
  expect(syntax.intentText).toBe(
    String.raw`Find $PATH @tag(plain) #literal \folder`,
  );
  expect(syntax.cues).toHaveLength(2);
  expect(canonical(parse(canonical(syntax)))).toBe(canonical(syntax));
  expect(syntax.preferences.map((p) => p.negative)).toEqual([false, true]);
});
it("requires intent and rejects removed roots, unknown and malformed islands", () => {
  for (const text of [
    "",
    " ",
    "*",
    '"question"',
    '("a" || "b")',
    '@tag("x")',
    "#migration",
    "question $unknown",
    'question @e("Alice"',
    "question $limit(2,,3)",
    "question $limit(2) $limit(3)",
    '+"school"',
    "intent @object(opaque)",
  ])
    expect(() => parse(text), text).toThrow();
});
it("binds names exactly, sorts participants, keeps text and rejects duplicate identity", async () => {
  const result = await compileNousQL(
    'Recall approvals @e("Bob","Alice") $return(memory) $prefer("school")',
    resolve,
    now,
  );
  expect(result.boundCanonical).toContain(
    "@e(ent:fasid-jizih-kudah,ent:rofas-nagal-lifod)",
  );
  expect(result.expression.cues.map((c) => c.cue.case)).toEqual([
    "text",
    "entityRef",
    "entityRef",
  ]);
  expect(result.expression.children).toEqual([]);
  await expect(
    compileNousQL('Recall approvals @e("Alice","Alice")', resolve, now),
  ).rejects.toThrow("DUPLICATE_ENTITY_PARTICIPANT");
});
it("keeps independent time axes and explicit soft time preferences", async () => {
  const result = await compileNousQL(
    "study $time(observed,within=30d) $effort(deep) $prefer(recent,formed) $avoid(recent,observed)",
    resolve,
    now,
  );
  const observed =
    result.expression.modifiers?.constraints?.observed?.predicate;
  expect(observed?.case).toBe("range");
  expect(observed?.case === "range" && observed.value.end?.seconds).toBe(
    now.seconds,
  );
  expect(result.expression.modifiers?.constraints?.occurred).toBeUndefined();
  expect(result.expression.modifiers?.preferences.map((p) => p.key)).toEqual([
    "recent:formed",
    "recent:observed",
  ]);
  await expect(
    compileNousQL('study $time(valid,at="2026-09-17")', resolve, now),
  ).rejects.toThrow("offset");
});
it("keeps omitted limits configurable and freezes historical name resolution", async () => {
  const cuts: (string | undefined)[] = [];
  const resolver = async (
    kind: string,
    locator: { value: string },
    cut?: Timestamp,
  ) => {
    cuts.push(cut ? toJson(TimestampSchema, cut) : undefined);
    return resolve(kind, locator);
  };
  const result = await compileNousQL(
    'Past policy @tag("Past") $asof(ago=1d)',
    resolver,
    now,
  );
  expect(cuts).toEqual(["2026-09-16T00:00:00Z"]);
  expect(result.expression.modifiers?.limit).toBeUndefined();
});

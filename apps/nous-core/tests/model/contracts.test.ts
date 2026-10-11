// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { expect, it } from "vitest";
import { structuredContractForRole } from "../../src/model/schemas/contracts.js";

it("query concept output accepts only local keys and bounded ephemeral hypotheses", () => {
  const schema = structuredContractForRole("query_concept_enrichment")!.owner;
  expect(
    schema.safeParse({
      existing_tags: [{ key: "c0", strength: 0.8 }],
      novel_concepts: [{ text: "reader reclamation" }],
    }).success,
  ).toBe(true);
  for (const existing_tags of [
    [{ key: "tag:invented", strength: 1 }],
    [{ key: "c0", strength: 2 }],
    [{ key: "c0", strength: 0.8, tag_id: "invented" }],
  ]) {
    expect(
      schema.safeParse({ existing_tags, novel_concepts: [] }).success,
    ).toBe(false);
  }
});

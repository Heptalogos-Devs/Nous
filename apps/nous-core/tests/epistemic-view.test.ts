// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { create } from "@bufbuild/protobuf";
import { expect, test } from "vitest";
import {
  ContextSegmentSchema,
  DegradationSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { epistemicView } from "../src/cognition/epistemic.js";

test("a selected knowledge stance keeps limitations and reports separate from fresh assistance", () => {
  const stance = create(ContextSegmentSchema, {
    authority: "subject_cognition",
    domain: "self",
    text: "Only heard the exam announcement",
    sourceRefs: [{ kind: "self_facet_revision", value: "known-revision" }],
    domainKnowledge: {
      domainRef: "topic:exam",
      familiarity: "aware",
      canDo: ["repeat announcement"],
      limitations: ["not practiced"],
      experienceKinds: ["heard"],
    },
  });
  const report = create(ContextSegmentSchema, {
    authority: "subject_cognition",
    domain: "memory",
    text: "A classmate reports a date",
    sourceRefs: [{ kind: "memory_revision", value: "report-revision" }],
    epistemicClass: "reported",
  });
  const worker = create(ContextSegmentSchema, {
    authority: "evidence",
    domain: "runtime",
    text: "A newly released worked solution",
    sourceRefs: [{ kind: "occurrence", value: "actual-handoff" }],
  });
  const omitted = create(DegradationSchema, {
    code: "context_budget_omission",
    detail: "Some optional sources omitted",
  });
  const view = epistemicView([stance, report, worker], [omitted]);
  expect(view.familiarDomains[0]?.domainKnowledge?.familiarity).toBe("aware");
  expect(view.unknowns[0]?.domainKnowledge?.limitations).toEqual([
    "not practiced",
  ]);
  expect(view.acceptedFactsAndReports[0]?.epistemicClass).toBe("reported");
  expect(view.executionAssistance[0]?.text).toContain("newly released");
  expect(view.executionAssistance[0]?.attribution).toContain(
    "does not establish prior",
  );
  expect(view.familiarDomains).toHaveLength(1);
  expect(view.complete).toBe(false);
  expect(view.coverage[0]?.code).toBe("context_budget_omission");
  expect(epistemicView([], []).familiarDomains).toEqual([]);
});

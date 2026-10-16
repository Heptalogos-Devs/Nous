// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { expect, test } from "vitest";
import { timestampFromDate } from "@bufbuild/protobuf/wkt";
import {
  criterionFactContext,
  decodeCriterionFact,
} from "../src/motivation.js";
import { providerContractForRole } from "../../../apps/nous-core/src/model/schemas/contracts.js";
test("official fact codec keeps domain event time distinct from actual recorded time", () => {
  const at = timestampFromDate(new Date("2030-01-01T00:00:00Z"));
  const recordedAt = timestampFromDate(new Date("2026-10-16T00:00:00Z"));
  const context = criterionFactContext({
    fact: {
      case: "event",
      value: {
        version: 1,
        eventType: "study_completed",
        target: "topic:one",
        at: { timeDomainRef: "time:course", at, recordedAt },
      },
    },
  });
  expect(decodeCriterionFact(context.criterion_fact).fact).toMatchObject({
    case: "event",
    value: {
      eventType: "study_completed",
      at: { timeDomainRef: "time:course", at, recordedAt },
    },
  });
  expect(() =>
    criterionFactContext({
      fact: {
        case: "measurement",
        value: {
          version: 1,
          name: "temperature",
          unit: "K",
          value: 4,
          error: -1,
          at: { timeDomainRef: "time:real", at, recordedAt },
        },
      },
    }),
  ).toThrow(/error/);
});
test("criterion is a fixed operation of the existing binding with its actual schema digest", () => {
  const reflection = providerContractForRole("subject_reflection")!;
  const criterion = providerContractForRole(
    "subject_reflection",
    "motivation_criterion",
  )!;
  expect(criterion.digest).not.toBe(reflection.digest);
  expect(
    criterion.owner.parse({
      results: [
        {
          key: "actual",
          truth: "unknown",
          reason: "No actual complete evidence",
          evidenceKeys: [],
        },
      ],
    }),
  ).toBeDefined();
  expect(() =>
    criterion.owner.parse({ results: [], complete: true }),
  ).toThrow();
  expect(() =>
    providerContractForRole("memory_formation", "motivation_criterion"),
  ).toThrow(/binding/);
});

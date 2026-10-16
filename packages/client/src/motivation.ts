// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import {
  create,
  fromJson,
  toJson,
  type MessageInitShape,
  type JsonValue,
} from "@bufbuild/protobuf";
import { CriterionFactSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/motivation_pb.js";
/** Canonical typed facts on actual ObservationInput context. Source provenance,
 * observation time and SourceRead remain the real observation owner's contract. */
export function criterionFactContext(
  value: MessageInitShape<typeof CriterionFactSchema>,
) {
  const fact = create(CriterionFactSchema, value);
  if (!fact.fact.case || fact.fact.value.version !== 1)
    throw new Error("An actual criterion fact requires a known typed version");
  const stamp = fact.fact.value.at;
  if (!stamp?.timeDomainRef || !stamp.at || !stamp.recordedAt)
    throw new Error(
      "An actual criterion fact needs domain time and real recording time",
    );
  if (
    fact.fact.case === "measurement" &&
    (!Number.isFinite(fact.fact.value.value) ||
      !Number.isFinite(fact.fact.value.error) ||
      fact.fact.value.error < 0)
  )
    throw new Error(
      "Measurement value/error must be finite with nonnegative error",
    );
  return { criterion_fact: toJson(CriterionFactSchema, fact) };
}
export const decodeCriterionFact = (json: JsonValue) =>
  fromJson(CriterionFactSchema, json);

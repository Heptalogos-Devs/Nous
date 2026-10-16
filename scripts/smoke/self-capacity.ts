// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import type { NousClient } from "@nous-wave/client";
import { create } from "@bufbuild/protobuf";
import { RevisionBasisSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import {
  SelfContentSchema,
  type SelfRecord,
} from "@nous-wave/protocol/nous/wave/v1alpha1/self_pb.js";

/** Capacity data is created only through official owner APIs in a caller's isolated Subject. */
export async function populateSelfCapacity(
  client: NousClient,
  subjectId: string,
) {
  const source = await client.runtime.recordObservation({
    subjectId,
    requestId: crypto.randomUUID(),
    sourceClass: "generated",
    material: {
      case: "inlineText",
      value: {
        text: "Synthetic capacity fixture: historical horticulture, unrelated to current identity and task.",
        mediaType: "text/plain",
      },
    },
    admit: false,
  });
  const basis = create(RevisionBasisSchema, {
    basis: {
      case: "evidence",
      value: {
        occurrenceId: source.occurrenceId,
        locator: { case: "wholeOccurrence", value: true },
        basisRole: "direct",
      },
    },
  });
  const work = await client.runtime.createWorkContext({
    subjectId,
    operationId: crypto.randomUUID(),
    purpose: "Unrelated synthetic capacity scope",
  });
  const records: SelfRecord[] = [];
  for (let index = 0; index < 121; index++) {
    const adopted = await client.self.adopt({
      subjectId,
      operationId: crypto.randomUUID(),
      reason: "Isolated synthetic capacity fixture",
      content: create(SelfContentSchema, {
        content: {
          case: "facet",
          value: {
            kind: "preference",
            statement: `Synthetic horticulture capacity item ${index}`,
            epistemicClass: "narrative",
            stance: "tentative",
            scope: {
              description: "Only the synthetic horticulture fixture",
              conversationRef:
                index >= 96 && index < 108 ? "capacity:unrelated" : undefined,
              purposes:
                index >= 84 && index < 96 ? ["professional_execution"] : [],
              workContextId:
                index >= 108 && index < 120
                  ? work.workContext!.workContextId
                  : undefined,
            },
            basis: [basis],
            validTime:
              index === 120
                ? {
                    timeDomainRef: "time:real",
                    extent: {
                      value: {
                        case: "interval",
                        value: { end: { seconds: 946684800n, nanos: 0 } },
                      },
                    },
                  }
                : { timeDomainRef: "time:real", extent: {} },
          },
        },
      }),
    });
    let record = await client.self.addEvidence({
      subjectId,
      operationId: crypto.randomUUID(),
      targetRevision: adopted.reference,
      expectedObjectEpoch: adopted.objectEpoch,
      role: "boundary_case",
      basis,
    });
    if (index < 84)
      record = await client.self.setLifecycle({
        mutation: {
          subjectId,
          operationId: crypto.randomUUID(),
          objectRef: record.objectRef,
          expectedObjectEpoch: record.objectEpoch,
        },
        action: "withdraw",
      });
    records.push(record);
  }
  return { records, basis };
}

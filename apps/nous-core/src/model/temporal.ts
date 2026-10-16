// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { create } from "@bufbuild/protobuf";
import { timestampFromDate } from "@bufbuild/protobuf/wkt";
import {
  TemporalExtentSchema,
  DomainTemporalExtentSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
export function temporal(
  v:
    | { kind: "unknown" }
    | { kind: "instant"; at: string }
    | { kind: "interval"; start: string | null; end: string | null },
) {
  return create(TemporalExtentSchema, {
    value:
      v.kind === "unknown"
        ? undefined
        : v.kind === "instant"
          ? { case: "instant", value: timestampFromDate(new Date(v.at)) }
          : {
              case: "interval",
              value: {
                start: v.start
                  ? timestampFromDate(new Date(v.start))
                  : undefined,
                end: v.end ? timestampFromDate(new Date(v.end)) : undefined,
              },
            },
  });
}

export function domainTemporal(
  v: Parameters<typeof temporal>[0] & { timeDomainRef?: string },
) {
  return create(DomainTemporalExtentSchema, {
    timeDomainRef: v.timeDomainRef ?? "time:real",
    extent: temporal(v),
  });
}

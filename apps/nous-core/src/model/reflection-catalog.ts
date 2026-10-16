// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { create, fromJson } from "@bufbuild/protobuf";
import { timestampFromDate } from "@bufbuild/protobuf/wkt";
import { Code, ConnectError } from "@connectrpc/connect";
import {
  RevisionBasisSchema,
  TemporalExtentSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import type { ReflectionPlan } from "./subject-reflection-plan.js";
function invalid(message: string): never {
  throw new ConnectError(message, Code.InvalidArgument);
}
export function chosenBasis(
  plan: ReflectionPlan,
  keys: string[],
  target?: string,
) {
  if (
    !keys.some((k) => plan.sourceKeys.includes(k)) ||
    new Set(keys).size !== keys.length ||
    keys.includes(target ?? "")
  )
    invalid(
      "A reflection mutation must cite this batch's actual new source; no recursive self support",
    );
  return keys.map((k) => {
    const b = new Map(plan.basis).get(k);
    return b
      ? fromJson(RevisionBasisSchema, b)
      : invalid("Reflection basis key is outside actual supplied catalog");
  });
}
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

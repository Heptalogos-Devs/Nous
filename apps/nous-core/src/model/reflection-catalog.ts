// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { fromJson } from "@bufbuild/protobuf";
import { Code, ConnectError } from "@connectrpc/connect";
import { RevisionBasisSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
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

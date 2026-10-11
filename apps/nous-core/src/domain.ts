// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import type { Message } from "@bufbuild/protobuf";
import type { CognitiveRef } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";

export type Ref = Omit<CognitiveRef, keyof Message>;
type Requirement = "REQUIRED" | "PREFERRED" | "OPTIONAL" | "FORBIDDEN";
export interface ConsumerPolicy {
  consumerId: string;
  revision: string;
  memory: Requirement;
  runtime: Requirement;
  resource: Requirement;
  maxItems: number;
  maxTextBytes: number;
  materialize: boolean;
}
export function refKey(ref: Ref): string {
  return ref.kind + "\0" + ref.value;
}

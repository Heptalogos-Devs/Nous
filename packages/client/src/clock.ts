// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { timestampDate } from "@bufbuild/protobuf/wkt";
import type {
  DomainStamp,
  SubjectClock,
} from "@nous-wave/protocol/nous/wave/v1alpha1/clock_pb.js";
export type ClockDeadline =
  { kind: "due"; at: Date } | { kind: "paused" } | { kind: "unmapped" };
/** Reproject a world due time after read/configure/pause/resume. No timer or model work starts here. */
export function projectWorldDeadline(
  clock: SubjectClock,
  due: DomainStamp,
): ClockDeadline {
  if (
    !due.timeDomainRef ||
    !Number.isFinite(clock.rate) ||
    clock.rate < 0 ||
    due.timeDomainRef !== clock.timeDomainRef ||
    !due.at ||
    !clock.now?.at ||
    !clock.now.recordedAt
  )
    return { kind: "unmapped" };
  const remaining =
    timestampDate(due.at).getTime() - timestampDate(clock.now.at).getTime();
  const real = timestampDate(clock.now.recordedAt).getTime();
  if (remaining <= 0) return { kind: "due", at: new Date(real) };
  if (clock.rate === 0) return { kind: "paused" };
  const at = real + remaining / clock.rate;
  if (!Number.isFinite(at) || !Number.isFinite(new Date(at).getTime()))
    return { kind: "unmapped" };
  return { kind: "due", at: new Date(at) };
}
/** Timestamp equality across different domains carries no temporal ordering. */
export function compareDomainStamps(
  first: DomainStamp,
  second: DomainStamp,
): number | undefined {
  if (
    !first.timeDomainRef ||
    first.timeDomainRef !== second.timeDomainRef ||
    !first.at ||
    !second.at
  )
    return undefined;
  if (first.at.seconds !== second.at.seconds)
    return first.at.seconds < second.at.seconds ? -1 : 1;
  return Math.sign(first.at.nanos - second.at.nanos);
}

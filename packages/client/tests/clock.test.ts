// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { create } from "@bufbuild/protobuf";
import { timestampFromDate } from "@bufbuild/protobuf/wkt";
import { expect, test } from "vitest";
import { SubjectClockSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/clock_pb.js";
import { DomainStampSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { compareDomainStamps, projectWorldDeadline } from "../src/index.js";

test("world due projection changes with rate and pause, and never infers a cross-world mapping", () => {
  const real = new Date("2026-10-16T00:00:00Z");
  const now = create(DomainStampSchema, {
    timeDomainRef: "time:course",
    at: timestampFromDate(real),
    recordedAt: timestampFromDate(real),
  });
  const due = create(DomainStampSchema, {
    ...now,
    at: timestampFromDate(new Date(real.getTime() + 600_000)),
  });
  const clock = create(SubjectClockSchema, {
    timeDomainRef: now.timeDomainRef,
    rate: 60,
    now,
  });
  expect(projectWorldDeadline(clock, due)).toEqual({
    kind: "due",
    at: new Date(real.getTime() + 10_000),
  });
  clock.rate = 0;
  expect(projectWorldDeadline(clock, due)).toEqual({ kind: "paused" });
  expect(projectWorldDeadline(clock, now)).toEqual({ kind: "due", at: real });
  due.timeDomainRef = "time:other";
  expect(projectWorldDeadline(clock, due)).toEqual({ kind: "unmapped" });
  expect(compareDomainStamps(now, due)).toBeUndefined();
  now.timeDomainRef = due.timeDomainRef = "";
  expect(compareDomainStamps(now, due)).toBeUndefined();
});

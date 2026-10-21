// Adapted from Nous-Wave master 412b0620: core/time.rs and client/clock.ts.
// Copyright 2026 Aravine Zhu; original pure algorithms Apache-2.0.
import { Temporal } from '@js-temporal/polyfill';
import {
  InstantInputSchema,
  TimePointInputSchema,
  TimeRangeInputSchema,
  type TimePoint,
  type TimeRange,
} from '@nous-wave/contracts/common';

export function parse(input: unknown): TimePoint {
  return TimePointInputSchema.parse(input);
}
export function parseRange(input: unknown): TimeRange {
  return TimeRangeInputSchema.parse(input);
}
export function parseInstant(input: unknown): string {
  return InstantInputSchema.parse(input);
}
export function readReal(): TimePoint {
  return {
    domainId: 'real',
    at: Temporal.Now.instant()
      .round({ smallestUnit: 'microsecond', roundingMode: 'floor' })
      .toString({ fractionalSecondDigits: 6 }),
  };
}

/** Original compareDomainStamps: different domains carry no ordering. */
export function compare(
  first: TimePoint,
  second: TimePoint,
): -1 | 0 | 1 | undefined {
  if (first.domainId !== second.domainId) return undefined;
  return Temporal.Instant.compare(first.at, second.at) as -1 | 0 | 1;
}

export interface DomainRange {
  domainId: string;
  range: TimeRange;
}
/** Port of TimeInterval.contains; endpoints remain half-open and optional. */
export function contains(
  interval: DomainRange,
  point: TimePoint,
): boolean | undefined {
  if (interval.domainId !== point.domainId) return undefined;
  const { start, end } = interval.range;
  if (
    (start && start.domainId !== interval.domainId) ||
    (end && end.domainId !== interval.domainId)
  )
    return undefined;
  return (
    (!start || Temporal.Instant.compare(start.at, point.at) <= 0) &&
    (!end || Temporal.Instant.compare(point.at, end.at) < 0)
  );
}
/** Port of TimeInterval.overlaps; touching endpoints do not overlap. */
export function overlaps(
  first: DomainRange,
  second: DomainRange,
): boolean | undefined {
  if (first.domainId !== second.domainId) return undefined;
  if (
    [
      first.range.start,
      first.range.end,
      second.range.start,
      second.range.end,
    ].some((point) => point && point.domainId !== first.domainId)
  )
    return undefined;
  return (
    (!first.range.start ||
      !second.range.end ||
      Temporal.Instant.compare(first.range.start.at, second.range.end.at) <
        0) &&
    (!first.range.end ||
      !second.range.start ||
      Temporal.Instant.compare(second.range.start.at, first.range.end.at) < 0)
  );
}

export {
  floorDiv,
  micros,
  formatMicros,
  rateParts,
  scaleMicros,
  addMicros,
  normalizeRate,
  projectDeadline,
  delaySecondsCeil,
} from './clock.js';

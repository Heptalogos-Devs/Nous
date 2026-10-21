// Copyright 2026 Aravine Zhu; SPDX-License-Identifier: Apache-2.0
// Adapted from Nous master packages/client/src/clock.ts and research/adapted-time/clock.ts.
import { Temporal } from '@js-temporal/polyfill';
import type { TimePoint } from '@nous-wave/contracts/common';
import { ClockRateSchema, type ClockDeadline } from '@nous-wave/contracts/time';
import { parseInstant } from './index.js';
export const floorDiv = (value: bigint, base: bigint) =>
  value >= 0n ? value / base : (value - (base - 1n)) / base;
export const micros = (at: string) =>
  floorDiv(Temporal.Instant.from(parseInstant(at)).epochNanoseconds, 1000n);
export function formatMicros(value: bigint): string {
  return parseInstant(
    Temporal.Instant.fromEpochNanoseconds(value * 1000n).toString({
      fractionalSecondDigits: 6,
    }),
  );
}
export function rateParts(rate: string) {
  const canonical = ClockRateSchema.parse(rate);
  const [whole, fraction = ''] = canonical.split('.');
  return {
    numerator: BigInt(whole + fraction),
    denominator: 10n ** BigInt(fraction.length),
  };
}
export function scaleMicros(elapsed: bigint, rate: string): bigint {
  const { numerator, denominator } = rateParts(rate);
  return floorDiv(elapsed * numerator, denominator);
}
export function addMicros(at: string, delta: bigint): string {
  return formatMicros(micros(at) + delta);
}
export function normalizeRate(rate: string): string {
  if (!/^(?:0|[1-9]\d{0,17})(?:\.\d{1,12})?$/.test(rate))
    throw new RangeError('Invalid clock rate');
  return rate.includes('.') ? rate.replace(/0+$/, '').replace(/\.$/, '') : rate;
}
export function projectDeadline(
  clock: {
    domainId: string;
    rate: string;
    now: { point: TimePoint; realAt: string };
  },
  due: TimePoint,
): ClockDeadline {
  if (
    !due.domainId ||
    due.domainId !== clock.domainId ||
    clock.now.point.domainId !== clock.domainId
  )
    return { kind: 'unmapped' };
  try {
    const remaining = micros(due.at) - micros(clock.now.point.at),
      real = micros(clock.now.realAt);
    if (remaining <= 0n) return { kind: 'due', at: formatMicros(real) };
    const { numerator, denominator } = rateParts(clock.rate);
    if (numerator === 0n) return { kind: 'paused' };
    const elapsed = (remaining * denominator + numerator - 1n) / numerator;
    return { kind: 'due', at: formatMicros(real + elapsed) };
  } catch {
    return { kind: 'unmapped' };
  }
}
/** DBOS seconds accept millisecond resolution. Round toward a later wakeup. */
export function delaySecondsCeil(realDue: string, realNow: string): number {
  const remaining = micros(realDue) - micros(realNow);
  if (remaining <= 0n) return 0;
  const ms = (remaining + 999n) / 1000n;
  if (ms > BigInt(Number.MAX_SAFE_INTEGER))
    throw new RangeError('Deadline overflow');
  return Number(ms) / 1000;
}

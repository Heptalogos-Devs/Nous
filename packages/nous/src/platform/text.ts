import type { Selector } from '@nous-wave/contracts/common';
import { fail } from './errors.js';

export function selectText(text: string, selector: Selector) {
  if (selector.kind !== 'text' || selector.within) fail('capability_unavailable', '当前纯文本表示不支持所选复合区域');
  const points = Array.from(text);
  const start = selector.start ?? 0;
  const end = selector.end ?? points.length;
  if (!Number.isInteger(start) || !Number.isInteger(end) || start < 0 || end < start || end > points.length) fail('invalid_input', '文本区域超出 Unicode code point 范围');
  return { text: points.slice(start, end).join(''), selector: { ...selector, start, end } };
}

import {
  type Problem,
  ProblemError,
  ProblemSchema,
  readProblem,
} from '@nous-wave/contracts/common';

const statusByCategory: Record<Problem['category'], number> = {
  validation: 400,
  forbidden: 403,
  not_found: 404,
  conflict: 409,
  unavailable: 503,
  resource_pressure: 503,
  budget: 429,
  effect_unknown: 409,
  failed: 500,
};

export function problem(
  code: string,
  category: Problem['category'],
  message: string,
  fields: Partial<Omit<Problem, 'code' | 'category' | 'message'>> = {},
): ProblemError {
  return new ProblemError({
    code,
    category,
    status: statusByCategory[category],
    messageKey: code,
    message,
    ...fields,
  });
}

export function safeProblem(error: unknown, code = 'system.failed'): Problem {
  const known = readProblem(error);
  if (known) return known;
  return {
    code,
    category: 'failed',
    status: 500,
    messageKey: code,
    message: '操作失败',
  };
}

export type { Problem };
export { ProblemError, ProblemSchema };

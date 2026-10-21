import { ProblemError, type Problem } from '@nous-wave/contracts/common';
type DomainCode = 'invalid_input'|'forbidden'|'not_found'|'conflict'|'capability_unavailable'|'budget_exhausted'|'dependency_unavailable'|'effect_unknown';
const categories:Record<DomainCode,Problem['category']>={invalid_input:'validation',forbidden:'forbidden',not_found:'not_found',conflict:'conflict',capability_unavailable:'unavailable',budget_exhausted:'budget',dependency_unavailable:'unavailable',effect_unknown:'effect_unknown'};
export class DomainError extends ProblemError {
  constructor(code: DomainCode, message: string, public details: { currentRevision?: number; operationId?: string } = {}) {
    super({code,category:categories[code],message,messageKey:`nous.${code}`,
      status:({invalid_input:400,forbidden:403,not_found:404,conflict:409,capability_unavailable:422,budget_exhausted:429,dependency_unavailable:503,effect_unknown:409})[code],...details});
  }
}
export function fail(code: DomainCode, message: string, details?: { currentRevision?: number; operationId?: string }): never { throw new DomainError(code,message,details); }

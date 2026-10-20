import type { Problem } from '@nous-wave/contracts/common';
export class DomainError extends Error {
  constructor(public code: Problem['code'], message: string, public details: { currentRevision?: number; operationId?: string } = {}) { super(message); }
  get status() { return ({invalid_input:400,forbidden:403,not_found:404,conflict:409,capability_unavailable:422,budget_exhausted:429,dependency_unavailable:503,effect_unknown:409})[this.code]; }
  problem(): Problem { return {type:`urn:nous:problem:${this.code}`,title:this.message,status:this.status,code:this.code,...this.details}; }
}
export function fail(code: Problem['code'], message: string, details?: { currentRevision?: number; operationId?: string }): never { throw new DomainError(code,message,details); }

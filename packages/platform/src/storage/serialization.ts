import { DBOS } from '@dbos-inc/dbos-sdk';
import {
  JsonValueSchema,
  type Problem,
  ProblemError,
  ProblemSchema,
  readProblem,
} from '@nous-wave/contracts/common';

let registered = false;

/** DBOS serializes thrown Errors to records before invoking its SuperJSON recipes. */
export function registerProblemSerialization(): void {
  if (registered) return;
  DBOS.registerSerialization({
    name: 'platform.ProblemError',
    isApplicable: (value): value is { problem: Problem } =>
      readProblem(value) !== undefined,
    serialize: (value) =>
      JsonValueSchema.parse(
        JSON.parse(JSON.stringify(ProblemSchema.parse(value.problem))),
      ),
    deserialize: (value) => new ProblemError(ProblemSchema.parse(value)),
  });
  registered = true;
}

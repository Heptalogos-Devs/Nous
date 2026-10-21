import {
  type Query,
  type QueryInput,
  QuerySchema,
} from '@nous-wave/contracts/cognition';

/** Current structured retrieval input. The formal NousQL language belongs to the query owner. */
export function resolveQuery(input: QueryInput): Query {
  return QuerySchema.parse(
    typeof input === 'string' ? { intent: input } : input,
  );
}

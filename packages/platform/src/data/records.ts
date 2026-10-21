// Immutable export cuts use owner-private rows; this file only handles PostgreSQL record I/O.
import { sql, type Kysely, type RawBuilder } from 'kysely';
import type { CallContext, Ref } from '@nous-wave/contracts/common';
import { problem } from '../errors.js';
import { SubjectIdSchema } from '@nous-wave/contracts/common';
export interface RecordStore {
  product: 'nous' | 'hept'; installationId: string; readonly db: Kysely<any>;
  authorize(ctx:CallContext,operation:string,target?:Ref,db?:Kysely<any>):Promise<void>;
}
function requireSubject(ctx:CallContext):asserts ctx is CallContext&{subjectId:string}{SubjectIdSchema.parse(ctx.subjectId);}

export interface SelectedRecord { collection: string; record: Record<string, unknown> }
export interface OwnerCollection {
  table: string;
  /** SQL predicate uses the actual collection row as `t`. */
  scope(subjectId: string): RawBuilder<boolean>;
  stringColumns?: readonly string[];
  validate(record: Record<string, unknown>, subjectId: string): void;
}
export interface RecordDataOwner {
  name: string;
  schemaVersion: 1;
  prepareExport(ctx: CallContext, selectionId: string, db: Kysely<any>): Promise<{ count: string }>;
  readSelection(ctx: CallContext, selectionId: string): AsyncIterable<SelectedRecord>;
  validate(ctx: CallContext, row: SelectedRecord): void;
  restore(ctx: CallContext, rows: AsyncIterable<SelectedRecord>, db: Kysely<any>): Promise<void>;
  deleteSubject(ctx: CallContext, db: Kysely<any>): Promise<{ count: string }>;
  release(selectionId: string, db?: Kysely<any>): Promise<void>;
}
export function subjectScope(subjectId: string) { return sql<boolean>`t.subject_id=${subjectId}::uuid`; }
export function createRecordDataOwner(
  store: RecordStore,
  options: {
    name: string;
    collections: readonly OwnerCollection[];
    beforeRead?(ctx: CallContext, row: SelectedRecord): Promise<void>;
    beforeDelete?(ctx: CallContext, db: Kysely<any>): Promise<void>;
  },
): RecordDataOwner {
  const members = `${options.name}_export_members`;
  const collections = new Map(options.collections.map(value => [value.table, value]));
  const ref = (ctx: CallContext, id: string): Ref => ({ product: store.product, installationId: store.installationId, subjectId: ctx.subjectId, kind: 'ExportSelection', id });
  function validate(ctx: CallContext, row: SelectedRecord) {
    requireSubject(ctx);
    const collection = collections.get(row.collection);
    if (!collection || !row.record || typeof row.record !== 'object' || Array.isArray(row.record))
      throw problem('data.collection_invalid', 'validation', '归档记录不属于该领域owner');
    collection.validate(row.record, ctx.subjectId);
  }
  return {
    name: options.name, schemaVersion: 1,
    async prepareExport(ctx, selectionId, db) {
      requireSubject(ctx); await store.authorize(ctx, 'data.export', ref(ctx, selectionId), db);
      for (const collection of options.collections) {
        let value = sql`to_jsonb(t)`;
        for (const column of collection.stringColumns ?? [])
          value = sql`${value} || jsonb_build_object(${column},${sql.ref(`t.${column}`)}::text)`;
        await sql`insert into ${sql.table(members)}(selection_id,collection,record)
          select ${selectionId}::uuid,${collection.table},${value}
          from ${sql.table(collection.table)} t where ${collection.scope(ctx.subjectId)}`.execute(db);
      }
      const result = await sql<{ count: string }>`select count(*)::text as count from ${sql.table(members)} where selection_id=${selectionId}::uuid`.execute(db);
      return result.rows[0]!;
    },
    async *readSelection(ctx, selectionId) {
      requireSubject(ctx); let cursor = '0';
      for (;;) {
        await store.authorize(ctx, 'data.export', ref(ctx, selectionId));
        const page = await sql<{ ordinal: string; collection: string; record: Record<string, unknown> }>`select ordinal::text,collection,record from ${sql.table(members)} where selection_id=${selectionId}::uuid and ordinal>${cursor}::bigint order by ordinal limit 100`.execute(store.db);
        for (const row of page.rows) {
          validate(ctx, row); await options.beforeRead?.(ctx, row);
          yield { collection: row.collection, record: row.record };
          cursor = row.ordinal;
        }
        if (page.rows.length < 100) return;
      }
    },
    validate,
    async restore(ctx, rows, db) {
      requireSubject(ctx); await store.authorize(ctx, 'data.import', undefined, db);
      for await (const row of rows) {
        validate(ctx, row);
        const collection=collections.get(row.collection)!;
        const result=await sql`insert into ${sql.table(row.collection)} select t.* from jsonb_populate_record(null::${sql.table(row.collection)},${JSON.stringify(row.record)}::jsonb) t where ${collection.scope(ctx.subjectId)}`.execute(db);
        if(result.numAffectedRows!==1n)throw problem('data.record_scope','validation','归档记录的实际父对象不属于目标主体');
      }
    },
    async deleteSubject(ctx, db) {
      requireSubject(ctx); await store.authorize(ctx, 'data.purge', undefined, db);
      await options.beforeDelete?.(ctx, db); let count = 0n;
      for (const collection of [...options.collections].reverse()) {
        const result = await sql`delete from ${sql.table(collection.table)} t where ${collection.scope(ctx.subjectId)}`.execute(db);
        count += result.numAffectedRows ?? 0n;
      }
      return { count: count.toString() };
    },
    async release(selectionId, db = store.db) {
      await sql`delete from ${sql.table(members)} where selection_id=${selectionId}::uuid`.execute(db);
    },
  };
}

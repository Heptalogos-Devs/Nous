// Source projection extracted from the existing organization owner; no SQL or implicit consumption.
import {
  canonicalRefKey,
  type CallContext,
  type Ref,
  type TimePoint,
} from '@nous-wave/contracts/common';
import type {
  MaterialView,
  ExperiencePage,
  ExperienceListSchema,
} from '@nous-wave/contracts/materials';
import type { KnowledgeView } from '@nous-wave/contracts/knowledge';
import type { z } from 'zod';
import { requireSubject } from '../platform/security.js';
import { fail } from '../platform/errors.js';
import { selectText } from '../platform/text.js';
export interface SourceDocument {
  ref: Ref;
  text: string;
  at?: TimePoint;
}
export interface SourceDocumentDependencies {
  materials: {
    read(ctx: CallContext, input: { ref: Ref }): Promise<MaterialView>;
  };
  encounters: {
    experiences(
      ctx: CallContext,
      input: z.input<typeof ExperienceListSchema>,
    ): Promise<ExperiencePage>;
  };
  knowledge: {
    read(
      ctx: CallContext,
      input: { id: string; revision?: number },
    ): Promise<KnowledgeView>;
  };
}
const unique = (refs: Ref[]) => [
  ...new Map(refs.map((ref) => [canonicalRefKey(ref), ref])).values(),
];
export function createSourceDocuments(deps: SourceDocumentDependencies) {
  async function sourceDocs(ctx: CallContext, refs: Ref[]) {
    requireSubject(ctx);
    const result: SourceDocument[] = [];
    for (const ref of unique(refs)) {
      if (ref.kind === 'Material') {
        const material = await deps.materials.read(ctx, { ref });
        if (material.text === undefined)
          fail('capability_unavailable', '该来源尚无本角色可使用的文本表示');
        result.push({
          ref: material.ref,
          text: material.text,
          at: material.occurred?.start,
        });
      } else if (ref.kind === 'ExperienceItem') {
        const experience = (
          await deps.encounters.experiences(ctx, { refs: [ref], limit: 1 })
        ).items[0];
        if (!experience) fail('not_found', '主体经历不存在');
        const inputs = await sourceDocs(
          ctx,
          experience.portions.map((portion) => portion.ref),
        );
        result.push({
          ref,
          text: inputs.map((input) => input.text).join('\n'),
          at: experience.observed,
        });
      } else {
        const view = await deps.knowledge.read(ctx, {
          id: ref.id,
          revision: ref.revision,
        });
        const selected = ref.selector
          ? selectText(view.body.content, ref.selector)
          : { text: view.body.content, selector: undefined };
        result.push({
          ref: { ...view.ref, selector: selected.selector },
          text: selected.text,
          at: view.valid?.start ?? view.formed,
        });
      }
    }
    return result;
  }

  return sourceDocs;
}
export type SourceDocuments = ReturnType<typeof createSourceDocuments>;

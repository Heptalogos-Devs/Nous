// Role list and Prompt mapping adapted from Nous-Wave 412b0620 model/roles.ts and prompts.ts.
// Copyright 2026 Aravine Zhu; original source Apache-2.0 (see LICENSES/Apache-2.0.txt).

import type {
  ModelKind,
  RoleDescriptor,
  RolePolicy,
} from '@nous-wave/platform/models';
import type { PromptDescriptor } from '@nous-wave/platform/prompts';
import { z } from 'zod';

const definitions: ReadonlyArray<{
  id: string;
  label: string;
  description: string;
  kind: ModelKind;
  prompt?: string;
  structured?: boolean;
}> = [
  {id:'native_agent',label:'原生执行器',description:'OpenClaw实际模型调用，原生owner保留执行与Prompt',kind:'language'},
  {
    id: 'projection_steward',
    label: '认知投影维护',
    description: '维护当前认知投影的有效范围与实际依据',
    kind: 'language',
    prompt: 'projection/steward.md',
    structured: true,
  },
  {
    id: 'memory_formation',
    label: '记忆形成',
    description: '从已取得资料形成可修订的有来源记忆',
    kind: 'language',
    prompt: 'memory/formation.md',
    structured: true,
  },
  {
    id: 'self_formation',
    label: '自我种子解释',
    description: '解释明确采用的种子，保留叙事与实际经历的区别',
    kind: 'language',
    prompt: 'self/formation.md',
    structured: true,
  },
  {
    id: 'self_reflection',
    label: '自我反思',
    description: '根据实际证据修正有范围的自我认识',
    kind: 'language',
    prompt: 'self/reflection.md',
    structured: true,
  },
  {
    id: 'subject_reflection',
    label: '主体认知反思',
    description: '在已启用领域中解释实际观察与当前认识',
    kind: 'language',
    prompt: 'subject/reflection.md',
    structured: true,
  },
  {
    id: 'episode_segmentation',
    label: '经历组织',
    description: '按实际成员和轨道组织经历分段',
    kind: 'language',
    prompt: 'episode/segmentation.md',
    structured: true,
  },
  {
    id: 'journal_synthesis',
    label: '日记形成',
    description: '从历史知情范围内的经历组织主体视角日记',
    kind: 'language',
    prompt: 'journal/synthesis.md',
    structured: true,
  },
  {
    id: 'memory_consolidation',
    label: '记忆整合',
    description: '保留来源独立性、适用边界与反例的认知整合',
    kind: 'language',
    prompt: 'memory/consolidation.md',
    structured: true,
  },
  {
    id: 'concept_maintenance',
    label: '概念维护',
    description: '根据实际概念、成员和证据提出概念维护',
    kind: 'language',
    prompt: 'memory/concept-maintenance.md',
    structured: true,
  },
  {
    id: 'material_description',
    label: '材料描述',
    description: '忠实描述实际可访问材料及其缺失范围',
    kind: 'language',
    prompt: 'material/description.md',
  },
  {
    id: 'material_structuring',
    label: '材料解释',
    description: '将已有描述组织为保留来源的结构化材料',
    kind: 'language',
    prompt: 'material/structure.md',
    structured: true,
  },
  {
    id: 'material_direct_structuring',
    label: '原件直接解释',
    description: '直接理解本次可访问的原件与上下文',
    kind: 'language',
    prompt: 'material/direct-structure.md',
    structured: true,
  },
  {
    id: 'query_embedding',
    label: '查询向量',
    description: '为正式查询计划计算向量通道输入',
    kind: 'embedding',
  },
  {
    id: 'query_concept_enrichment',
    label: '查询概念增强',
    description: '以查询局部假设增强召回，保留概念身份边界',
    kind: 'language',
    prompt: 'query/concept-enrichment.md',
    structured: true,
  },
  {
    id: 'query_rerank',
    label: '候选模型重排',
    description: '通过原生重排模型对已召回候选排序',
    kind: 'reranking',
  },
  {
    id: 'speech_transcription',
    label: '语音转写',
    description: '转写实际取得音频并保留媒体来源',
    kind: 'transcription',
  },
  {
    id: 'video_description',
    label: '视频描述',
    description: '保留实际帧、音轨、采样间隔和时间标签的描述',
    kind: 'language',
    prompt: 'material/video-description.md',
  },
  {
    id: 'motivation_criterion',
    label: '动机条件判断',
    description: '逐条判断实际观察对固定条件的支持、反证或未知',
    kind: 'language',
    prompt: 'motivation/criterion.md',
    structured: true,
  },
];
const promptId = (path: string) => path.slice(0, -3).replaceAll('/', '.');
function defaultPolicy(kind: ModelKind, prompt?: string): RolePolicy {
  const role = {
    enabled: false,
    resources: [],
    selection: 'ordered' as const,
    timeoutMs: 60_000,
    ...(prompt ? { prompt: { promptId: promptId(prompt), locale: 'en' } } : {}),
  };
  switch (kind) {
    case 'language':
      return {
        ...role,
        kind,
        inference: { maxOutputTokens: 4096, providerOptions: {} },
      };
    case 'embedding':
      return {
        ...role,
        kind,
        inference: { maxParallelCalls: 4, providerOptions: {} },
      };
    case 'reranking':
      return { ...role, kind, inference: { topN: 12, providerOptions: {} } };
    case 'transcription':
      return { ...role, kind, inference: { providerOptions: {} } };
    case 'speech':
      return { ...role, kind, inference: { providerOptions: {} } };
  }
}
export const roleCatalog: readonly RoleDescriptor[] = definitions.map(
  (role) => ({
    id: role.id,
    label: role.label,
    description: role.description,
    kind: role.kind,
    requiredCapabilities: role.id==='native_agent'?['input:text']:role.structured?['structured']:[],
    defaultPolicy: defaultPolicy(role.kind, role.prompt),
    ...(role.prompt ? { promptId: promptId(role.prompt) } : {}),
  }),
);
/** programRoot is the installed @nous-wave/nous package root. Runtime data is supplied in SDK messages. */
export const promptCatalog: readonly PromptDescriptor[] = definitions.flatMap(
  (role) =>
    role.prompt
      ? [
          {
            id: promptId(role.prompt),
            owner: role.id,
            labelKey: `prompts.${role.id}.label`,
            descriptionKey: `prompts.${role.id}.description`,
            visibility: 'advanced' as const,
            locales: ['en'],
            defaultLocale: 'en',
            programFiles: { en: `prompts/en/${role.prompt}` },
            paramsSchema: z.strictObject({}),
            paramsSchemaRevision: '1',
            examples: { en: {} },
            allowedPartials: [],
          },
        ]
      : [],
);

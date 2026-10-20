import { createHash, randomBytes } from 'node:crypto';
import { z } from 'zod';
import { NousClient } from '@nous-wave/client';
import { ContextPrepareSchema, ContextViewSchema } from '@nous-wave/contracts/cognition';
import { EncounterAcceptSchema, MaterialViewSchema } from '@nous-wave/contracts/materials';
import { InvocationPrepareSchema, InvocationCompletionSchema } from '@nous-wave/contracts/management';
import { RefSchema, type Consumer, type Ref } from '@nous-wave/contracts/common';

const binding = z.object({
  subjectId: z.string(), consumerId: z.string().min(1), consumer: z.enum(['subject', 'worker', 'author', 'maintenance']).default('worker'),
  purpose: z.string().default('worker'), audience: z.array(z.string()).default([]),
  billing: z.enum(['nous', 'external']).default('nous'),
});
export const configSchema = z.object({
  baseURL: z.url(), tokenEnv: z.string().default('NOUS_SERVICE_TOKEN'), installationId: z.string(),
  bindings: z.record(z.string(), binding), modelResources: z.record(z.string(), z.string()),
});
type AgentContext = { agentId?: string; sessionKey?: string; parentSessionKey?: string; sessionId?: string; runId?: string };
export type PrepareEvent = {
  traceparent?: string; role?: string;
  callId: string; runId: string; sessionId: string; provider: string; model: string;
  contextLimit: number; outputTokens: number; inputTokensEstimate: number; transcriptCursor?: string;
  systemPrompt?: string; messages: unknown[]; tools: unknown[];
};
type Prepared = { view: z.infer<typeof ContextViewSchema>; systemPromptAddition?: string; inputRefs?: Ref[] };
export type AcceptEvent = {
  traceparent?: string; role?: string;
  callId: string; runId: string; sessionId: string; prepared: { view: unknown };
  outcome?: 'not_sent';
  output?: { role: string; content: unknown[]; timestamp: number; usage?: unknown; provider?: string; model?: string; responseId?: string; stopReason?: string; diagnostics?: { type: string }[]; modelContext?: unknown };
  transcriptEntryId?: string;
};
type DispatchEvent = Omit<AcceptEvent, 'output' | 'transcriptEntryId'> & {
  provider: string; model: string; inputTokensEstimate: number; outputTokens: number;
};
export interface NativePluginApi {
  pluginConfig?: unknown;
  runtime: {
    modelContext?: { version: number; harnesses: readonly string[] };
    gateway: { request<T>(method: string, input: Record<string, unknown>): Promise<T> };
  };
  on(name: 'model_context_prepare', handler: (event: PrepareEvent, context: AgentContext) => Promise<Prepared | undefined>): void;
  on(name: 'model_context_accept', handler: (event: AcceptEvent, context: AgentContext) => Promise<{ outputRef: Ref; selectedOutputRefs: Ref[] } | undefined>): void;
  on(name: 'model_context_dispatch', handler: (event: DispatchEvent, context: AgentContext) => Promise<void>): void;
}

const now = () => ({ clockId: 'real', at: new Date().toISOString() });
const textOf = (value: unknown): string => typeof value === 'string' ? value : JSON.stringify(value);
const traceHeader = (event: { runId: string; traceparent?: string }) => event.traceparent ?? `00-${createHash('sha256').update(event.runId).digest('hex').slice(0, 32)}-${randomBytes(8).toString('hex')}-01`;
function latestNativePortion(messages: unknown[]): string {
  const previousResponse = messages.findLastIndex(message => typeof message === 'object' && message !== null && 'role' in message && message.role === 'assistant');
  const current = messages.slice(previousResponse + 1);
  const rendered = JSON.stringify(current.length ? current : messages.slice(-1), function (key, value) {
    if (key === 'data' && typeof value === 'string' && this.type === 'image') return '[native image payload]';
    return value;
  });
  return [...rendered].slice(0, 8000).join('');
}
function currentNativeMessages(messages: unknown[]): unknown[] {
  const previousResponse = messages.findLastIndex(message => typeof message === 'object' && message !== null && 'role' in message && message.role === 'assistant');
  const current = messages.slice(previousResponse + 1);
  return current.length ? current : messages.slice(-1);
}

export function registerNousCognition(api: NativePluginApi) {
  if (api.runtime.modelContext?.version !== 1) throw new Error('Nous requires the source-built OpenClaw model context handoff');
  const config = configSchema.parse(api.pluginConfig);
  const token = process.env[config.tokenEnv];
  if (!token) throw new Error(`Missing service credential environment variable ${config.tokenEnv}`);

  const client = new NousClient({ baseURL: config.baseURL, apiKey: token });

  async function resolve(context: AgentContext) {
    // Agent identity comes from the native host; model output cannot select it.
    let parent = context.parentSessionKey;
    const visited = new Set<string>();
    while (parent && visited.size < 32 && !visited.has(parent)) {
      visited.add(parent);
      const agentId = /^agent:([^:]+):/.exec(parent)?.[1];
      if (agentId && config.bindings[agentId]) return config.bindings[agentId];
      const result = await api.runtime.gateway.request<{ session: { parentSessionKey?: string; spawnedBy?: string } | null }>('sessions.describe', { key: parent });
      parent = result.session?.parentSessionKey ?? result.session?.spawnedBy;
    }
    return context.agentId ? config.bindings[context.agentId] : undefined;
  }
  async function post(resource: string, payload: unknown, event: { callId: string; runId: string; traceparent?: string }, current: z.infer<typeof binding>, suffix: string): Promise<unknown> {
    return client.request('POST', `/v1/subjects/${encodeURIComponent(current.subjectId)}/${resource}`, payload, {
      submissionId: `${event.callId}:${suffix}`, purpose: current.purpose, audience: current.audience,
      traceparent: traceHeader(event),
    });
  }
  const source = (event: { callId: string; sessionId: string }, subjectId: string, stage: string): Ref => ({
    product: 'nous', installationId: config.installationId, subjectId,
    kind: 'openclaw.transcript', id: `${event.sessionId}:${event.callId}:${stage}`,
  });

  api.on('model_context_prepare', async (event, context) => {
    const current = await resolve(context);
    if (!current) return undefined;
    const consumer: Consumer = { kind: current.consumer, host: 'openclaw', id: current.consumerId };
    const modelResourceId = config.modelResources[`${event.provider}/${event.model}`];
    if (!modelResourceId) throw new Error(`Register and map the actual model resource ${event.provider}/${event.model}`);
    const inputText = latestNativePortion(event.messages);
    const requiredSources = [...new Map(event.messages.flatMap(message => {
      if (!message || typeof message !== 'object' || !('modelContext' in message)) return [];
      const metadata = message.modelContext;
      if (!metadata || typeof metadata !== 'object' || !('inputRefs' in metadata) || !Array.isArray(metadata.inputRefs)) return [];
      return [...metadata.inputRefs, ...('outputRef' in metadata && metadata.outputRef ? [metadata.outputRef] : [])].map(value => RefSchema.parse(value));
    }).map(ref => [JSON.stringify(ref), ref])).values()];
    const origin = source(event, current.subjectId, 'input');
    const document: Ref = { ...origin, kind: 'openclaw.session', id: event.sessionId };
    const material = MaterialViewSchema.parse(await post('materials', {
      kind: 'native_model_input', mediaType: 'text/plain', text: inputText,
      source: { ref: origin, roots: [document], originVersion: event.transcriptCursor ?? event.callId },
    }, event, current, 'input'));
    const latestMaterials: z.input<typeof ContextPrepareSchema>['latestMaterials'] = [{ ref: material.ref, text: inputText, required: true }];
    let imageIndex = 0;
    for (const message of currentNativeMessages(event.messages)) {
      if (!message || typeof message !== 'object' || !('content' in message) || !Array.isArray(message.content)) continue;
      for (const part of message.content) {
        if (!part || typeof part !== 'object' || part.type !== 'image' || typeof part.data !== 'string' || typeof part.mimeType !== 'string') continue;
        const bytes = Buffer.from(part.data, 'base64');
        const digest = createHash('sha256').update(bytes).digest('hex');
        const blob = await client.uploadBlob(current.subjectId, bytes, part.mimeType, {
          submissionId: `${event.sessionId}:image:${digest}`, purpose: current.purpose, audience: current.audience,
          traceparent: traceHeader(event),
        });
        const image = MaterialViewSchema.parse(await post('materials', {
          kind: 'native_image_input', mediaType: part.mimeType, blobRef: blob.ref,
          source: { ref: { ...origin, selector: { kind: 'part', partId: `image:${imageIndex}` } }, roots: [document], originVersion: event.transcriptCursor ?? event.callId },
        }, event, current, `image:${imageIndex++}`));
        latestMaterials.push({ ref: image.ref, text: `[Native image: ${part.mimeType}]`, selector: { kind: 'image', x: 0, y: 0, width: 1, height: 1 }, required: true });
      }
    }
    const request = ContextPrepareSchema.parse({
      consumer, role: event.role ?? current.consumer, roleTask: '',
      situation: '',
      query: inputText.slice(0, 4000),
      latestMaterials,
      requiredSources,
      model: { resourceId: modelResourceId, modelId: event.model, contextLimit: event.contextLimit },
      budget: { outputTokens: event.outputTokens,
        toolTokens: 0,
        requiredTokens: Math.max(0, event.inputTokensEstimate - [...inputText].length) },
      observed: now(),
    });
    const view = ContextViewSchema.parse(await post('context-views', request, event, current, 'prepare'));
    // Native inputs and role instructions are already present. Only the selected
    // cognition occupies this ephemeral slot; the host applies it after compaction.
    const systemPromptAddition = view.blocks.filter(block => block.kind !== 'task' && block.kind !== 'sources')
      .map(block => block.text).join('\n\n');
    return { view, systemPromptAddition, inputRefs: view.inputPortions.map(portion => ({ ...portion.ref, ...(portion.selector ? { selector: portion.selector } : {}) })) };
  });

  api.on('model_context_dispatch', async (event, context) => {
    const current = await resolve(context);
    if (!current || current.billing === 'external') return;
    const view = ContextViewSchema.parse(event.prepared.view);
    const modelResourceId = config.modelResources[`${event.provider}/${event.model}`];
    if (!modelResourceId) throw new Error('Actual model resource is not mapped');
    await post('model-invocations/prepare', InvocationPrepareSchema.parse({
      invocationId: event.callId, role: event.role ?? current.consumer, modelResourceId,
      inputTokensEstimate: event.inputTokensEstimate, maxOutputTokens: event.outputTokens,
      inputRefs: view.selectedRefs,
      nativeRunRef: { ...source(event, current.subjectId, 'run'), kind: 'openclaw.run', id: event.runId },
    }), event, current, 'invocation');
    await post(`model-invocations/${encodeURIComponent(event.callId)}/dispatch`, {}, event, current, 'dispatch');
  });

  api.on('model_context_accept', async (event, context) => {
    const current = await resolve(context);
    if (!current) return;
    const view = ContextViewSchema.parse(event.prepared.view);
    if (view.subjectId !== current.subjectId) throw new Error('Native context subject binding changed');
    if (event.outcome === 'not_sent') {
      if (current.billing === 'nous') {
        try { await post(`model-invocations/${encodeURIComponent(event.callId)}/completion`, { status: 'not_sent' }, event, current, 'completion'); }
        catch (error) { if (!(error && typeof error === 'object' && 'code' in error && error.code === 'not_found')) throw error; }
      }
      return;
    }
    if (!event.output) throw new Error('Native completed model output is missing');
    const origin = source(event, current.subjectId, 'output');
    const { modelContext: _nativeMetadata, ...providerOutput } = event.output;
    const output = MaterialViewSchema.parse(await post('materials', {
      kind: 'native_model_output', mediaType: 'application/json', text: textOf(providerOutput),
      source: { ref: origin, roots: [], inputs: view.inputPortions.map(portion => ({ ...portion.ref, ...(portion.selector ? { selector: portion.selector } : {}) })), originVersion: event.callId },
    }, event, current, 'output'));
    const usage = z.record(z.string(), z.unknown()).parse(event.output.usage ?? {});
    const succeeded = event.output.stopReason !== 'error' && event.output.stopReason !== 'aborted';
    const refused = event.output.diagnostics?.some(diagnostic => diagnostic.type === 'provider_refusal');
    if (current.billing === 'nous') await post(`model-invocations/${encodeURIComponent(event.callId)}/completion`, InvocationCompletionSchema.parse({
      status: succeeded ? 'completed' : refused ? 'not_sent' : 'unknown',
      inputTokens: typeof usage.input === 'number' ? usage.input + Number(usage.cacheRead ?? 0) + Number(usage.cacheWrite ?? 0) : undefined, outputTokens: usage.output,
      cacheReadTokens: usage.cacheRead, cacheWriteTokens: usage.cacheWrite,
      providerRequestId: event.output.responseId, outputRef: output.ref, rawUsage: usage,
    }), event, current, 'completion');
    const selectedOutputRefs = event.output.content.flatMap((part, index) => part && typeof part === 'object' && 'type' in part && part.type === 'text' && 'text' in part && typeof part.text === 'string' && part.text.trim()
      ? [{ ...output.ref, selector: { kind: 'node' as const, nodeId: `/content/${index}/text` } }] : []);
    if (!succeeded) return { outputRef: output.ref, selectedOutputRefs };
    const request = EncounterAcceptSchema.parse({
      consumer: view.consumer, invocationId: event.callId, step: 0, contextViewRef: view.ref,
      portions: [...view.inputPortions.map(part => ({ ref: part.ref })), ...view.selectedRefs.map(ref => ({ ref }))], resultRef: output.ref,
      observed: { clockId: 'real', at: new Date(event.output.timestamp).toISOString() }, occurred: { start: { clockId: 'real', at: new Date(event.output.timestamp).toISOString() } },
      purpose: current.purpose, completion: 'complete',
    });
    await post('encounters', request, event, current, 'accept');
    return { outputRef: output.ref, selectedOutputRefs };
  });
}

export default {
  id: 'nous-cognition', name: 'Nous Cognition',
  description: 'Prepare and accept cognition at each native model boundary',
  register: registerNousCognition,
};

// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

// Focused native fixture: evaluates the installed production functions, with only
// Host/protocol I/O replaced and receipts normalized locally. No QQ traffic is used.
import assert from "node:assert/strict";
import { mkdtemp, readFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import vm from "node:vm";
import { pathToFileURL } from "node:url";
import { tmpdir } from "node:os";

const native = resolve(process.argv[2] ?? "");
if (!process.argv[2])
  throw new Error("Pass the controlled OpenClaw package root");
const source = await readFile(
  join(native, "dist/deliver-prepare-7gjKTwQN.js"),
  "utf8",
);
const storage = await readFile(
  join(native, "dist/delivery-queue-storage-BmsyhVaX.js"),
  "utf8",
);
function section(text: string, start: string, end: string): string {
  const offset = text.indexOf(start);
  assert(offset >= 0, start);
  const finish = text.indexOf(end, offset + start.length);
  assert(finish > offset, end);
  return text.slice(offset, finish);
}
const custody = section(
  source,
  "async function resolveChannelDeliveryCustodyParams",
  "async function createChannelHandler",
);
const handler = section(
  source,
  "function createPluginHandler",
  "const createChannelOutboundContextBase",
);
const lifecycle = section(
  source,
  "async function runChannelMessageSendWithLifecycle",
  "async function resolveOutboundDurableFinalDeliverySupport",
);
const base = section(
  source,
  "const createChannelOutboundContextBase",
  "//#endregion",
);
const projection = section(
  storage,
  "function projectPreparedOutboundBatchForStorage",
  "function mapPreparedOutboundAcceptedPayloads",
);
const mapping = section(
  storage,
  "function mapPreparedOutboundAcceptedPayloads",
  "//#endregion",
);
const sends: unknown[] = [];
const checks: Array<Record<string, unknown>> = [];
let adoptions = 0;
let denied = false;
let feedbackUnavailable = false;
let cancellation: AbortController | undefined;
const message = {
  deliveryCustody: {
    async adopt(input: Record<string, unknown>) {
      adoptions++;
      assert.equal(input.runId, "native-run");
      assert.deepEqual(input.session, { key: "native-session" });
      return { reference: "host:adopted-behavior" };
    },
    async authorize(input: Record<string, unknown>) {
      checks.push(input);
      cancellation?.abort();
      assert.equal(
        (input.custody as { reference: string }).reference,
        "host:adopted-behavior",
      );
      if (denied || input.to === "1072706649")
        throw new Error("Host forbids send");
    },
  },
  send: {
    ...Object.fromEntries(
      ["text", "media", "payload"].map((kind) => [
        kind,
        async (input: unknown) => {
          sends.push(input);
          return { receipt: { platformMessageIds: ["fixture-id"] } };
        },
      ]),
    ),
    lifecycle: {
      async afterSendSuccess() {
        if (feedbackUnavailable) throw new Error("Host feedback unavailable");
      },
    },
  },
};
type Batch = Record<string, unknown> & {
  deliveryCustody?: { reference: string };
};
interface FixtureHandler {
  sendText(text: string, overrides?: Record<string, unknown>): Promise<unknown>;
  sendMedia(
    text: string,
    media: string,
    overrides?: Record<string, unknown>,
  ): Promise<unknown>;
  sendPayload(payload: Record<string, unknown>): Promise<unknown>;
}
interface NativeFixture {
  normalize(params: Record<string, unknown>): Promise<Record<string, unknown>>;
  adopt(
    params: Record<string, unknown>,
    batch: Batch,
    options?: Record<string, unknown>,
  ): Promise<Batch>;
  handler(params: Record<string, unknown>): FixtureHandler;
  project(batch: Batch): Batch;
  map(batch: Batch, payloads: Array<Record<string, unknown>>): Batch;
}
const errors = (await import(
  pathToFileURL(join(native, "dist/deliver-types-Diy-VQKA.js")).href
)) as {
  r: new (
    message: string,
    options: { retryable?: boolean; cause?: unknown },
  ) => Error;
};
const fixture = vm.runInNewContext(
  `${custody}\n${lifecycle}\n${handler}\n${base}\n${projection}\n${mapping}\n({normalize:resolveChannelDeliveryCustodyParams,adopt:adoptChannelDeliveryCustody,handler:createPluginHandler,project:projectPreparedOutboundBatchForStorage,map:mapPreparedOutboundAcceptedPayloads})`,
  {
    structuredClone,
    PlatformMessageNotDispatchedError: errors.r,
    normalizeOutboundReplyFacts: (params: { reply?: unknown }) => params.reply,
    loadBootstrappedChannelPlugin: async () => ({ plugin: { message } }),
    log: { warn() {} },
    formatErrorMessage: String,
    normalizeChannelMessageSendResult: (_: unknown, result: unknown) => result,
    attachOutboundDeliveryCommitHook: (result: unknown) => result,
  },
) as NativeFixture;
const raw = {
  schemaVersion: 1,
  sourcePayloadCount: 1,
  runId: "native-run",
  entries: [
    {
      status: "accepted",
      sourceIndex: 0,
      payload: {
        text: "draft",
        deliveryCustody: { reference: "model-forgery" },
      },
    },
  ],
};
const params = {
  getSourceIndex: () => 0,
  channel: "qq",
  to: "1101497265",
  accountId: "bot",
  queuePolicy: "required",
  deliveryQueueId: "native-queue",
  session: { key: "native-session" },
  message,
};
const normalizedMessage = await fixture.normalize({
  ...params,
  deliveryIntentId: undefined,
  deliveryQueueId: undefined,
  queuePolicy: "best_effort",
  skipQueue: true,
  mirror: { idempotencyKey: "native-message-tool-call" },
});
assert.equal(normalizedMessage.skipQueue, false);
assert.equal(normalizedMessage.queuePolicy, "required");
assert.equal(
  normalizedMessage.deliveryIntentId,
  JSON.stringify(["channel-custody", "qq", "bot", "native-message-tool-call"]),
);
const recovering = await fixture.normalize({
  ...params,
  skipQueue: true,
  preparedBatch: { deliveryCustody: { reference: "host:adopted-behavior" } },
});
assert.equal(recovering.skipQueue, true);
await assert.rejects(
  fixture.normalize({
    ...params,
    deliveryQueueId: undefined,
    mirror: undefined,
  }),
  /stable producer intent/,
);
// A genuine session alone is a native producer identity; Host still validates it.
const noProvenance = { ...params, session: undefined };
await assert.rejects(
  fixture.adopt(noProvenance, { ...raw, runId: undefined }),
  /provenance/,
);
await assert.rejects(
  fixture.adopt({ ...params, skipQueue: true }, raw),
  /durable queue/,
);
await assert.rejects(
  fixture.adopt(params, raw, { allowDeliveryCustodyAdoption: false }),
  /recovery/,
);
const adopted = await fixture.adopt(params, raw);
assert.equal(adoptions, 1);
assert.equal(adopted.deliveryCustody?.reference, "host:adopted-behavior");
// Exercise the actual queue projection and media payload replacement, then JSON
// round-trip as SQLite does. The Host reference survives outside payload metadata.
const staged = fixture.project(fixture.map(adopted, [{ text: "staged" }]));
// Exercise the installed native SQLite queue, not a replacement queue.
const queue = (await import(
  pathToFileURL(join(native, "dist/delivery-queue-storage-BmsyhVaX.js")).href
)) as {
  r(
    input: Record<string, unknown>,
    id: string,
    stateDir: string,
  ): Promise<{ id: string; created: boolean }>;
  p(id: string, stateDir: string): Promise<{ preparedBatch: Batch } | null>;
};
const stateDir = await mkdtemp(join(tmpdir(), "nous-native-custody-"));
const queued = await queue.r(
  { ...params, preparedBatch: staged },
  "fixture-durable-intent",
  stateDir,
);
assert(queued.created);
const duplicate = await queue.r(
  { ...params, preparedBatch: staged },
  "fixture-durable-intent",
  stateDir,
);
assert.equal(duplicate.created, false);
const sparseBatch = {
  ...adopted,
  sourcePayloadCount: 8,
  entries: [
    { status: "accepted", sourceIndex: 2, payload: { text: "identical" } },
    { status: "accepted", sourceIndex: 7, payload: { text: "identical" } },
  ],
};
const sparseQueued = await queue.r(
  { ...params, preparedBatch: fixture.project(sparseBatch) },
  "sparse-source-identity",
  stateDir,
);
const sparseLoaded = await queue.p(sparseQueued.id, stateDir);
assert(sparseLoaded);
assert.deepEqual(
  (sparseLoaded.preparedBatch.entries as { sourceIndex: number }[]).map(
    (entry) => entry.sourceIndex,
  ),
  [2, 7],
  "SQLite recovery must retain original sparse payload indices, never renumber by body or array position",
);
const loaded = await queue.p(queued.id, stateDir);
assert(loaded);
const replayed = loaded.preparedBatch;
assert.equal(replayed.deliveryCustody?.reference, "host:adopted-behavior");
const pending = fixture.handler({
  ...params,
  preparedBatch: replayed,
  onPlatformSendStart: async () => {},
});
await pending.sendText("part one", {
  deliveryPartIndex: 0,
  deliveryPartCount: 2,
});
await pending.sendMedia("caption", "https://fixture.invalid/media", {
  deliveryPartIndex: 1,
  deliveryPartCount: 2,
});
await pending.sendPayload({ text: "typed payload" });
assert.equal(sends.length, 3);
assert.equal(checks.length, 3);
assert.equal(adoptions, 1, "replay must not adopt again");
assert.equal(
  (checks[1]!.payload as { mediaUrl: string }).mediaUrl,
  "https://fixture.invalid/media",
);
assert.equal(
  (checks[0]!.payload as { deliveryPartIndex: number }).deliveryPartIndex,
  0,
);
denied = true;
await assert.rejects(pending.sendText("cancelled"), /Host rejected/);
assert.equal(sends.length, 3);
denied = false;
const observer = fixture.handler({
  ...params,
  to: "1072706649",
  preparedBatch: replayed,
});
await assert.rejects(
  observer.sendMedia("forbidden", "https://fixture.invalid/image"),
  /Host rejected/,
);
assert.equal(sends.length, 3);
const forged = fixture.handler({ ...params, preparedBatch: raw });
await assert.rejects(forged.sendText("forged"), /custody/);
assert.equal(sends.length, 3);
cancellation = new AbortController();
const cancelledDuringAuthorization = fixture.handler({
  ...params,
  preparedBatch: replayed,
  abortSignal: cancellation.signal,
});
await assert.rejects(
  cancelledDuringAuthorization.sendText("cancelled during Host check"),
  /cancelled before dispatch/,
);
assert.equal(sends.length, 3);
cancellation = undefined;
feedbackUnavailable = true;
await pending.sendText("platform accepted despite feedback outage");
assert.equal(sends.length, 4, "failed Host feedback must not replay the send");
// Uncontrolled channels preserve the native adapter contract.
const generic = fixture.handler({ ...params, message: { send: message.send } });
await generic.sendText("generic");
assert.equal(sends.length, 5);
// Run the complete production delivery entry as a native message producer does:
// runtime-owned skipQueue=true plus an existing native idempotency key.
process.env.OPENCLAW_STATE_DIR = stateDir;
async function nativeModule<T>(name: string): Promise<T> {
  return (await import(pathToFileURL(join(native, "dist", name)).href)) as T;
}
const nativeRegistry = await nativeModule<{ t(): { channels: unknown[] } }>(
  "registry-empty-55wlVNzO.js",
);
const nativeScope = await nativeModule<{
  d<T>(registry: unknown, run: () => Promise<T>): Promise<T>;
}>("gateway-request-scope-CmMd3nUp.js");
const nativeDelivery = await nativeModule<{
  n(input: Record<string, unknown>): Promise<unknown[]>;
}>("deliver-B6F55ipQ.js");
// The native transcript, not a plugin queue, retains Host contact identity and
// actual results across reload and reset boundaries.
const nativeSessions = await nativeModule<{
  t: {
    open(target: Record<string, string>): {
      appendMessage(message: unknown): string;
      appendResetBoundary(reason: string): string;
      flushPendingPersistence(): void;
    };
  };
}>("session-manager-CNTG_BAJ.js");
const nativeTranscript = await nativeModule<{
  loadTranscriptEventsSync(target: Record<string, string>): unknown[];
}>("plugin-sdk/session-store-runtime.js");
const nativeTarget = {
  agentId: "host-fixture",
  sessionId: "host-feedback-history",
  sessionKey: "agent:host-fixture:main",
  storePath: join(stateDir, "sessions.json"),
};
const nativeSession = nativeSessions.t.open(nativeTarget);
const preservedContact = {
  contactRequestId: "native-contact",
  observedAt: new Date().toISOString(),
};
nativeSession.appendMessage({
  role: "toolResult",
  toolCallId: "already-executed",
  toolName: "write",
  content: [{ type: "text", text: "actual result" }],
  isError: false,
  timestamp: Date.now(),
  nousHostBridge: preservedContact,
});
const nativeAgentSource = await readFile(
  join(native, "dist/agent-core-BxgCPa8O.js"),
  "utf8",
);
const emitActualTool = vm.runInNewContext(`
  const copyInternalToolResultState=(_source,message)=>message;
  const withToolResultContentSource=(message)=>message;
  ${section(nativeAgentSource, "async function emitToolResultMessage(", "function readTurnTaintMetadata(")}
  emitToolResultMessage;
`) as (
  finalized: unknown,
  emit: (event: {
    type: string;
    message: Record<string, unknown>;
  }) => Promise<void>,
  managed: unknown,
) => Promise<unknown>;
let persistedBeforeFeedback = false;
await emitActualTool(
  {
    toolCall: { id: "strict-native-tool", name: "write" },
    isError: false,
    result: {
      content: [{ type: "text", text: "already performed" }],
      isError: false,
    },
  },
  async (event) => {
    if (event.type === "message_end") {
      assert.deepEqual(event.message.nousHostBridge, preservedContact);
      nativeSession.appendMessage(event.message);
      persistedBeforeFeedback = true;
    }
  },
  {
    captureEvent(actualMessage: Record<string, unknown>) {
      actualMessage.nousHostBridge = preservedContact;
    },
    async observe() {
      assert(
        persistedBeforeFeedback,
        "real tool result must reach native SQLite before network feedback",
      );
    },
  },
);
nativeSession.appendResetBoundary("reset");
nativeSession.flushPendingPersistence();
const retained = nativeTranscript.loadTranscriptEventsSync(nativeTarget) as {
  type?: string;
  message?: { nousHostBridge?: unknown };
}[];
assert(
  retained.some(
    (event) =>
      event.type === "message" &&
      JSON.stringify(event.message?.nousHostBridge) ===
        JSON.stringify(preservedContact),
  ),
  "SQLite reset must retain the only copy of an unacknowledged actual result",
);
const registry = nativeRegistry.t();
let producerAdoptions = 0;
let producerAuthorizations = 0;
let producerSends = 0;
const authorizedSources: (number | undefined)[] = [];
const receiptSources: (number | undefined)[] = [];
registry.channels.push({
  plugin: {
    id: "qq-fixture",
    meta: { id: "qq-fixture", label: "fixture" },
    capabilities: { chatTypes: ["direct"] },
    message: {
      deliveryCustody: {
        async adopt(input: { deliveryIntentId?: string; runId?: string }) {
          producerAdoptions++;
          assert(input.deliveryIntentId);
          assert.equal(input.runId, "trusted-native-run");
          return { reference: "host:full-behavior" };
        },
        async authorize(input: {
          to: string;
          deliveryQueueId?: string;
          custody: { reference: string };
          payload: { sourceIndex?: number };
        }) {
          producerAuthorizations++;
          authorizedSources.push(input.payload.sourceIndex);
          assert(input.deliveryQueueId);
          assert.equal(input.custody.reference, "host:full-behavior");
          if (input.to === "1072706649") throw new Error("receive-only");
        },
      },
      send: {
        async text(ctx: { sourceIndex?: number }) {
          producerSends++;
          receiptSources.push(ctx.sourceIndex);
          return {
            messageId: "fixture-platform-id",
            receipt: {
              parts: [],
              platformMessageIds: ["fixture-platform-id"],
              primaryPlatformMessageId: "fixture-platform-id",
              sentAt: Date.now(),
            },
          };
        },
      },
    },
  },
});
const producer = {
  cfg: {},
  channel: "qq-fixture",
  to: "allowed-peer",
  accountId: "fixture-account",
  session: { key: "trusted-native-session" },
  runId: "trusted-native-run",
  payloads: [{ text: "native message tool output" }],
  skipQueue: true,
  mirror: { idempotencyKey: "native-message-tool-call" },
};
const productionResult = await nativeScope.d(registry, () =>
  nativeDelivery.n(producer),
);
assert.equal(productionResult.length, 1);
assert.equal(producerAdoptions, 1);
assert.equal(producerAuthorizations, 1);
assert.equal(producerSends, 1);
await assert.rejects(
  nativeScope.d(registry, () => nativeDelivery.n(producer)),
  /already queued/,
);
assert.equal(
  producerAdoptions,
  1,
  "same native request cannot adopt or send twice",
);
assert.equal(producerSends, 1);
await assert.rejects(
  nativeScope.d(registry, () =>
    nativeDelivery.n({
      ...producer,
      to: "1072706649",
      mirror: { idempotencyKey: "native-observe-only-call" },
    }),
  ),
  /Host rejected/,
);
assert.equal(
  producerSends,
  1,
  "full native observer path must stop before protocol",
);
const productionBatch = await nativeScope.d(registry, () =>
  nativeDelivery.n({
    ...producer,
    payloads: [
      { text: "identical approved payload" },
      { text: "identical approved payload" },
    ],
    mirror: { idempotencyKey: "native-message-batch" },
  }),
);
assert.equal(productionBatch.length, 2);
assert.equal(producerSends, 3);
assert.equal(
  producerAuthorizations,
  4,
  "observer rejection plus every physical part is authorized",
);

assert.deepEqual(
  authorizedSources.slice(-2),
  [0, 1],
  "identical bodies retain distinct source payload authorization identity",
);
assert.deepEqual(
  receiptSources.slice(-2),
  [0, 1],
  "actual Channel send receives the same original source index for receipts",
);
await assert.rejects(
  nativeScope.d(registry, () =>
    nativeDelivery.n({
      ...producer,
      payloads: [
        { text: "identical approved payload" },
        { text: "identical approved payload" },
      ],
      mirror: { idempotencyKey: "native-message-batch" },
    }),
  ),
  /already queued/,
);
assert.equal(
  producerSends,
  3,
  "completed identical-payload batch must not resend on retry",
);

let restoredSourceIndex = 2;
const restoredSparseHandler = fixture.handler({
  ...params,
  preparedBatch: sparseLoaded!.preparedBatch,
  getSourceIndex: () => restoredSourceIndex,
});
await restoredSparseHandler.sendText("identical");
restoredSourceIndex = 7;
await restoredSparseHandler.sendText("identical");
assert.deepEqual(
  checks
    .slice(-2)
    .map((check) => (check.payload as { sourceIndex: number }).sourceIndex),
  [2, 7],
);
assert.deepEqual(
  sends.slice(-2).map((send) => (send as { sourceIndex: number }).sourceIndex),
  [2, 7],
  "recovered identical payload receipt identities must remain sparse original indices",
);

// Agent RPC final output must traverse the actual payload builder, transforms,
// durable sender, SQLite intent and Host custody, not just message-tool delivery.
const nativePayloadBuilder = await nativeModule<{
  t(input: Record<string, unknown>): Record<string, unknown>[];
}>("payloads-B7_zRmw4.js");
const nativeAgentDelivery = await nativeModule<{
  deliverAgentCommandResult(input: Record<string, unknown>): Promise<unknown>;
}>("delivery.runtime-CAsclR-9.js");
const appliedAssistant = {
  role: "assistant",
  content: [{ type: "text", text: "paid final fixture" }],
  stopReason: "stop",
  provider: "fixture",
  model: "fixture",
  timestamp: Date.now(),
  openclawDelivery: { textPhaseRequiresTerminal: true },
  nousHostBridge: {
    outcome: "completed",
    identity: {
      native: {
        runId: "trusted-native-run",
        sessionKey: "trusted-native-session",
      },
      invocationId: "trusted-native-run:nous:1",
    },
  },
};
const finalPayloads = nativePayloadBuilder.t({
  assistantTexts: ["paid final fixture"],
  currentAssistant: appliedAssistant,
  lastAssistant: appliedAssistant,
  runId: "trusted-native-run",
  config: {},
});
assert.equal(finalPayloads.length, 1);
const finalParams = {
  cfg: {},
  deps: {},
  runtime: { log() {}, error() {} },
  opts: {
    deliver: true,
    channel: "qq-fixture",
    accountId: "fixture-account",
    to: "allowed-peer",
    runId: "trusted-native-run",
    sessionKey: "trusted-native-session",
  },
  outboundSession: { key: "trusted-native-session", agentId: "host-fixture" },
  result: { meta: {} },
  payloads: finalPayloads,
};
const beforeFinal = producerSends;
const finalResult = await nativeScope.d(registry, () =>
  nativeAgentDelivery.deliverAgentCommandResult(finalParams),
);
assert.equal(
  (finalResult as { deliverySucceeded?: boolean }).deliverySucceeded,
  true,
);
assert.equal(producerSends, beforeFinal + 1);
assert.equal(authorizedSources.at(-1), 0);
assert.equal(receiptSources.at(-1), 0);
await assert.rejects(
  nativeScope.d(registry, () =>
    nativeAgentDelivery.deliverAgentCommandResult(finalParams),
  ),
  /already queued/,
);
assert.equal(
  producerSends,
  beforeFinal + 1,
  "same paid invocation cannot send twice",
);
await assert.rejects(
  nativeScope.d(registry, () =>
    nativeAgentDelivery.deliverAgentCommandResult({
      ...finalParams,
      payloads: [{ text: "unattributed final" }],
    }),
  ),
  /stable producer intent/,
);
assert.equal(producerSends, beforeFinal + 1);

console.log(
  JSON.stringify({
    custody: "PASS",
    adoptions,
    authorizedSends: sends.length - 1,
    nativePayloadSourceIdentity: "PASS",
    nativeContactHistoryRetention: "PASS",
    nativeSqliteStableIntent: "PASS",
    nativeMessageSkipQueueProducer: "PASS",
    nativeAgentRpcFinalProducer: "PASS",
    feedbackFailurePreservesSend: "PASS",
    rejectedBeforeProtocol: 4,
    qqTraffic: 0,
  }),
);

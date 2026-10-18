// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { createHash } from "node:crypto";

/** @typedef {{runId:string,sessionId:string,sessionKey:string,agentId:string,historyGeneration:string}} NativeIdentity */
/** @typedef {{attemptId:string,stage:string,sequence:number,native:NativeIdentity,recordedAt:string,[field:string]:unknown}} StoredRecord */
/** @typedef {{type:string,customType?:string,data?:unknown}} NativeEntry */
/** @typedef {{getEntries():NativeEntry[],appendCustomEntry(type:string,data:unknown):unknown,flushPendingPersistence():void}} NativeSession */
/** @typedef {{name?:unknown,status?:unknown,code?:unknown}} Failure */
const customType = "nous_provider_attempt";
/** @param {unknown} value */
const fingerprint = (value) =>
  createHash("sha256").update(JSON.stringify(value)).digest("hex");
/** Uses the native transcript transaction; custom entries never become model messages.
 * @param {NativeSession} sessionManager
 * @param {NativeIdentity} input
 * @param {{subjectId:string,invocationId:string}} application
 */
export function createNousProviderCustody(sessionManager, input, application) {
  if (
    !application?.subjectId ||
    !application.invocationId ||
    !input?.runId ||
    !input.sessionId
  )
    throw Error("Required native provider custody identity is unavailable");
  const records = () =>
    sessionManager
      .getEntries()
      .filter(
        (entry) => entry.type === "custom" && entry.customType === customType,
      )
      .map((entry) => /** @type {StoredRecord} */ (entry.data));
  /** @param {Record<string,unknown> & {attemptId:string,stage:string,recordedAt:string}} record */
  const save = (record) => {
    const previous = records().find(
      (entry) =>
        entry.attemptId === record.attemptId && entry.stage === record.stage,
    );
    if (previous && fingerprint(previous) !== fingerprint(record))
      throw Error("Native provider receipt identity changed");
    if (!previous) sessionManager.appendCustomEntry(customType, record);
    sessionManager.flushPendingPersistence();
  };
  return {
    /** @param {unknown} payload @param {{provider:string,id:string,api:string}} model */
    async request(payload, model) {
      const body = /** @type {unknown} */ (JSON.parse(JSON.stringify(payload)));
      if (
        !body ||
        typeof body !== "object" ||
        Array.isArray(body) ||
        ["apiKey", "api_key", "authorization", "headers"].some((key) =>
          Object.hasOwn(body, key),
        )
      )
        throw Error("Provider request body contains transport authority");
      const sequence =
        records()
          .filter(
            (entry) =>
              entry.stage === "request" && entry.native.runId === input.runId,
          )
          .reduce((last, entry) => Math.max(last, entry.sequence), 0) + 1;
      const attemptId = `${input.runId}:provider:${sequence}`;
      /** @type {{version:number,attemptId:string,sequence:number,subjectId:string,invocationId:string,native:NativeIdentity,contactRequestId?:string}} */
      const identity = {
        version: 1,
        attemptId,
        sequence,
        subjectId: application.subjectId,
        invocationId: application.invocationId,
        native: {
          runId: input.runId,
          sessionId: input.sessionId,
          sessionKey: input.sessionKey,
          agentId: input.agentId,
          historyGeneration: input.historyGeneration,
        },
      };
      const contact = sessionManager
        .getEntries()
        .filter(
          (entry) =>
            entry.type === "custom" &&
            entry.customType === "nous_admitted_contact",
        )
        .map(
          (entry) =>
            /** @type {{identity:{invocationId:string,contactRequestId:string}}} */ (
              entry.data
            ),
        )
        .findLast(
          (entry) => entry.identity.invocationId === application.invocationId,
        );
      if (contact)
        identity.contactRequestId = contact.identity.contactRequestId;
      save({
        ...identity,
        stage: "request",
        recordedAt: new Date().toISOString(),
        provider: model.provider,
        model: model.id,
        api: model.api,
        requestDigest: fingerprint(body),
        input: body,
      });
      /** @type {{status:number,requestId?:string}|undefined} */
      let response;
      let finished = false;
      return {
        attemptId,
        /** @param {{status:number,requestId?:string}} value */
        async response(value) {
          response = {
            status: value.status,
            ...(value.requestId ? { requestId: value.requestId } : {}),
          };
          const previous = records().find(
            (entry) =>
              entry.attemptId === attemptId && entry.stage === "response",
          );
          save({
            ...identity,
            stage: "response",
            recordedAt: previous?.recordedAt ?? new Date().toISOString(),
            response,
          });
        },
        /** @param {{outcome:string,usage?:unknown,responseId?:string,error?:Failure}} value */
        async finish(value) {
          if (finished) return;
          const status =
            typeof value.error?.status === "number" &&
            Number.isInteger(value.error.status)
              ? value.error.status
              : response?.status;
          const outcome =
            value.outcome === "completed"
              ? "completed"
              : status && status >= 400
                ? "failed"
                : "uncertain";
          const previous = records().find(
            (entry) =>
              entry.attemptId === attemptId && entry.stage === "terminal",
          );
          const terminal = {
            ...identity,
            stage: "terminal",
            recordedAt: previous?.recordedAt ?? new Date().toISOString(),
            outcome,
            ...(value.usage
              ? {
                  usage: /** @type {unknown} */ (
                    JSON.parse(JSON.stringify(value.usage))
                  ),
                }
              : {}),
            ...(value.responseId ? { responseId: value.responseId } : {}),
            ...(value.error
              ? {
                  error: {
                    name:
                      typeof value.error.name === "string"
                        ? value.error.name
                        : "ProviderError",
                    ...(status ? { status } : {}),
                    ...(typeof value.error.code === "string" &&
                    /^[\w.-]{1,128}$/.test(value.error.code)
                      ? { code: value.error.code }
                      : {}),
                  },
                }
              : {}),
          };
          save(terminal);
          finished = true;
        },
      };
    },
  };
}

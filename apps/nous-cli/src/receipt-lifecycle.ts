// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import type { NousClient } from "@nous-wave/client";
import { create } from "@bufbuild/protobuf";
import { CliError } from "./agent.js";
import { receiptSchema, type OperationReceipt } from "./state.js";
import {
  freezeRequest,
  freezeResult,
  receiptDefinitionCommands,
  receiptDefinition,
  receiptIdentity,
  safeOperationStatusIdentity,
  thawRequest,
  thawResult,
} from "./receipt-codec.js";
import { cliState } from "./state.js";
import type { connectNousInstance } from "@nous-wave/client/node";
import {
  ConfigurationMutationIdentitySchema,
  GetOperationStatusRequestSchema,
  MutationReceiptIdentitySchema,
  ObservationRequestIdentitySchema,
  OperationState,
  SubjectCreateIdentitySchema,
  type GetOperationStatusRequest,
} from "@nous-wave/protocol/nous/wave/v1alpha1/management_pb.js";
import { ErrorOwner } from "@nous-wave/protocol/nous/wave/v1alpha1/errors_pb.js";

type ConnectedClient = Awaited<ReturnType<typeof connectNousInstance>>;
type Local = Awaited<ReturnType<typeof cliState>>;
type Notice = { code: string; message: string; receipt?: string };
type ReceiptCommand = ReturnType<typeof receiptDefinitionCommands>[number];
const receiptInfo = (
  receipt: OperationReceipt,
  identity: ReturnType<typeof receiptIdentity>,
) => ({
  receiptId: receipt.receiptId,
  command: receipt.command,
  owner: identity.owner,
  operationKind: identity.operationKind,
  operationId: identity.operationId,
  ...(identity.subjectId ? { subjectId: identity.subjectId } : {}),
});
const withReceiptId = (error: unknown, receiptId: string) => {
  const failure =
    error instanceof Error
      ? error
      : new CliError("OPERATION_FAILED", "Operation failed");
  Object.assign(failure, { receiptId });
  return failure;
};

export function createReceiptLifecycle(
  original: ConnectedClient,
  local: Local,
  subjectId: string,
  notices: Notice[],
) {
  const replay: Record<string, (request: unknown) => Promise<unknown>> = {};
  function savedRequest(receipt: OperationReceipt) {
    const spec = receiptDefinition(receipt.command);
    const request = thawRequest(
      receipt.command,
      receipt.request.typeName,
      receipt.request.protoJson,
    );
    const identity = receiptIdentity(receipt.command, request);
    return { request, spec, identity };
  }
  async function executeReceipt(
    id: string,
    receipt: Extract<OperationReceipt, { status: "pending" }>,
    invoke: (request: unknown) => Promise<unknown>,
    saved = savedRequest(receipt),
  ) {
    const { request } = saved;
    let result: unknown;
    try {
      result = await invoke(request);
    } catch (error) {
      throw withReceiptId(error, id);
    }
    try {
      const frozenResult = freezeResult(receipt.command, result);
      await local.writeReceipt(id, {
        ...receipt,
        status: "complete",
        result: frozenResult,
        updatedAt: new Date().toISOString(),
      });
    } catch {
      notices.push({
        code: "RECEIPT_UNSAVED",
        receipt: id,
        message:
          "Operation returned successfully; its result is shown, but the local completion receipt could not be saved",
      });
    }
    return result;
  }
  function mutation<I extends object, O>(
    name: ReceiptCommand,
    fn: ((request: I) => Promise<O>) | undefined,
  ) {
    replay[name] = (request) => {
      if (!fn)
        throw new CliError(
          "INVALID_ARGUMENT",
          `Registered receipt client method unavailable: ${name}`,
        );
      return fn(request as I);
    };
    return async (request: I) => {
      if (!fn)
        throw new CliError(
          "INVALID_ARGUMENT",
          `Registered receipt client method unavailable: ${name}`,
        );
      const id = crypto.randomUUID();
      const frozen = freezeRequest(name, request);
      const createdAt = new Date().toISOString();
      const receipt = receiptSchema.parse({
        receiptId: id,
        command: name,
        request: frozen.frozen,
        status: "pending",
        createdAt,
        updatedAt: createdAt,
      }) as Extract<OperationReceipt, { status: "pending" }>;
      await local.writeReceipt(id, receipt);
      return (await executeReceipt(id, receipt, replay[name]!, frozen)) as O;
    };
  }
  const client: NousClient & Pick<typeof original, "artifacts"> = {
    ...original,
  };
  const clonedNamespaces = new Set<string>();
  for (const name of receiptDefinitionCommands()) {
    const [namespaceName, methodName] = name.split(".") as [
      keyof NousClient,
      string,
    ];
    const namespace = namespaceName;
    const source = original[namespace];
    if (!clonedNamespaces.has(namespaceName)) {
      Object.assign(client, { [namespaceName]: { ...source } });
      clonedNamespaces.add(namespaceName);
    }
    const method: unknown = source
      ? Reflect.get(source, methodName)
      : undefined;
    const invoke: ((request: object) => Promise<unknown>) | undefined =
      typeof method === "function"
        ? (request) =>
            Reflect.apply(method, source, [request]) as Promise<unknown>
        : undefined;
    Reflect.set(
      client[namespace],
      methodName,
      mutation<object, unknown>(name, invoke),
    );
  }
  async function readOperationStatus(
    request: GetOperationStatusRequest,
    receiptId?: string,
  ) {
    try {
      return operationStatusProjection(
        await original.system.operationStatus(request),
      );
    } catch (error) {
      if (receiptId) throw withReceiptId(error, receiptId);
      throw error;
    }
  }
  function operationStatusProjection(
    remote: Awaited<ReturnType<typeof original.system.operationStatus>>,
  ) {
    const serverState = OperationState[remote.state] ?? "UNKNOWN";
    const serverOwner = ErrorOwner[remote.owner] ?? "UNSPECIFIED";
    return {
      serverState,
      serverOwner,
      ...(remote.subjectId ? { serverSubjectId: remote.subjectId } : {}),
      results: remote.results.map((result) => ({
        reference: result.reference,
        ...(result.revisionId ? { revisionId: result.revisionId } : {}),
        ...(result.objectEpoch !== undefined
          ? { objectEpoch: result.objectEpoch.toString() }
          : {}),
      })),
      ...(remote.recordedAt ? { recordedAt: remote.recordedAt } : {}),
      ...(remote.updatedAt ? { updatedAt: remote.updatedAt } : {}),
      ...(remote.leaseUntil ? { leaseUntil: remote.leaseUntil } : {}),
      ...(remote.committedAt ? { committedAt: remote.committedAt } : {}),
    };
  }
  async function inspectOperation(receiptId: string) {
    const receipt = await local.receipt(receiptId);
    const { spec, identity: frozenIdentity } = savedRequest(receipt);
    const receiptSubjectId = frozenIdentity.subjectId ?? "";
    const statusLookup = spec.statusLookup;
    if (statusLookup === "unsupported")
      return {
        ...receiptInfo(receipt, frozenIdentity),
        localStatus: receipt.status,
      };
    let identity;
    switch (statusLookup) {
      case "subjectCreate":
        identity = {
          case: "subjectCreate" as const,
          value: create(SubjectCreateIdentitySchema, {
            subjectId: receiptSubjectId,
            operationId: frozenIdentity.operationId,
          }),
        };
        break;
      case "configurationMutation":
        identity = {
          case: "configurationMutation" as const,
          value: create(ConfigurationMutationIdentitySchema, {
            operationId: frozenIdentity.operationId,
          }),
        };
        break;
      case "observationRequest":
        identity = {
          case: "observationRequest" as const,
          value: create(ObservationRequestIdentitySchema, {
            subjectId: receiptSubjectId,
            requestId: frozenIdentity.operationId,
          }),
        };
        break;
      case "mutationReceipt":
        identity = {
          case: "mutationReceipt" as const,
          value: create(MutationReceiptIdentitySchema, {
            subjectId: receiptSubjectId,
            operationId: frozenIdentity.operationId,
            operationKind: frozenIdentity.operationKind,
          }),
        };
        break;
    }
    const projection = await readOperationStatus(
      create(GetOperationStatusRequestSchema, { identity }),
      receiptId,
    );
    return {
      ...receiptInfo(receipt, frozenIdentity),
      localStatus: receipt.status,
      ...projection,
    };
  }
  async function inspectOperationIdentity(request: GetOperationStatusRequest) {
    return {
      identity: safeOperationStatusIdentity(request.identity),
      ...(await readOperationStatus(request)),
    };
  }
  async function retry(id: string) {
    const receipt = await local.receipt(id);
    const saved = savedRequest(receipt);
    if (saved.identity.subjectId && saved.identity.subjectId !== subjectId)
      throw new CliError(
        "RESULT_SUBJECT_MISMATCH",
        "Use --subject with the Subject ID frozen in this operation receipt",
      );
    if (receipt.status === "complete")
      return thawResult(receipt.command, receipt.result);
    const invoke = replay[receipt.command];
    if (!invoke)
      throw new CliError("INVALID_ARGUMENT", "Receipt command is unavailable");
    return executeReceipt(id, receipt, invoke, saved);
  }
  return { client, retry, inspectOperation, inspectOperationIdentity };
}

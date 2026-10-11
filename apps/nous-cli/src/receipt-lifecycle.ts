// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { NousError, type NousClient } from "@nous-wave/client";
import { createHash } from "node:crypto";
import { create } from "@bufbuild/protobuf";
import { CliError } from "./agent.js";
import { receiptSchema, type OperationReceipt } from "./state.js";
import {
  freezeRequest,
  freezeResult,
  receiptDefinitionCommands,
  receiptDefinition,
  thawRequest,
  thawResult,
} from "./receipt-codec.js";
import { cliState } from "./state.js";
import type { connectNousInstance } from "@nous-wave/client/node";
import {
  ConfigurationMutationIdentitySchema,
  GetOperationStatusRequestSchema,
  ModelWorkflowIdentitySchema,
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

export function createReceiptLifecycle(
  original: ConnectedClient,
  local: Local,
  subjectId: string,
  notices: Notice[],
) {
  const replay: Record<string, (request: unknown) => Promise<unknown>> = {};
  async function executeReceipt(
    id: string,
    receipt: Extract<
      OperationReceipt,
      { status: "pending" | "outcome_unknown" }
    >,
    invoke: (request: unknown) => Promise<unknown>,
  ) {
    const request = thawRequest(
      receipt.command,
      receipt.request.typeName,
      receipt.request.protoJson,
    );
    let result: unknown;
    try {
      result = await invoke(request);
    } catch (error) {
      const receiptOwner = receiptDefinition(receipt.command).owner;
      const record =
        error && typeof error === "object"
          ? (error as Record<string, unknown>)
          : {};
      const errorCode =
        typeof record.errorCode === "string"
          ? record.errorCode
          : typeof record.code === "string"
            ? record.code
            : undefined;
      const errorOwner =
        typeof record.owner === "string" ? record.owner : undefined;
      const noCommit =
        error instanceof NousError &&
        error.semanticComplete &&
        !!errorCode &&
        ((errorOwner === receiptOwner &&
          receiptDefinition(receipt.command).noCommitCodes.includes(
            errorCode,
          )) ||
          (errorOwner === "KERNEL" && errorCode === "INVALID_REQUEST"));
      if (noCommit) {
        try {
          await local.writeReceipt(id, {
            ...receipt,
            status: "rejected",
            error: {
              ...(typeof record.transportCode === "number"
                ? { transportCode: record.transportCode }
                : {}),
              ...(errorCode ? { code: errorCode } : {}),
              owner: errorOwner,
              message: error.message.slice(0, 2048),
              ...(typeof record.recovery === "string"
                ? { recovery: record.recovery }
                : {}),
              context:
                record.context && typeof record.context === "object"
                  ? (record.context as Record<string, string>)
                  : {},
              ...(typeof record.traceId === "string"
                ? { traceId: record.traceId }
                : {}),
            },
            updatedAt: new Date().toISOString(),
          });
        } catch {
          Object.assign(error, {
            receiptId: id,
            certainty: "known_rejection",
            notices: [
              {
                code: "RECEIPT_UNSAVED",
                message:
                  "The operation was definitively rejected; the local terminal receipt could not be saved",
              },
            ],
          });
        }
        throw error;
      }
      try {
        await local.writeReceipt(id, {
          ...receipt,
          status: "outcome_unknown",
          updatedAt: new Date().toISOString(),
        });
      } catch {
        notices.push({
          code: "RECEIPT_UNSAVED",
          receipt: id,
          message:
            "The operation outcome is unknown and its local receipt could not be updated",
        });
      }
      const failure =
        error instanceof NousError
          ? error
          : new CliError(
              "OUTCOME_UNKNOWN",
              `Response unavailable; retry ${id} to reuse the saved operation`,
            );
      Object.assign(failure, { receiptId: id, certainty: "outcome_unknown" });
      throw failure;
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
      const spec = receiptDefinition(name);
      const frozen = freezeRequest(name, request);
      const createdAt = new Date().toISOString();
      const requestDigest = createHash("sha256")
        .update(JSON.stringify(frozen.frozen))
        .digest("hex");
      const receipt = receiptSchema.parse({
        format: "nous.consumer.operation.v2",
        receiptId: id,
        command: name,
        owner: spec.owner,
        operationKind: spec.operationKind,
        subjectId: frozen.subjectId,
        operationId: frozen.operationId,
        requestDigest,
        request: frozen.frozen,
        status: "pending",
        createdAt,
        updatedAt: createdAt,
      }) as Extract<OperationReceipt, { status: "pending" }>;
      await local.writeReceipt(id, receipt);
      return (await executeReceipt(id, receipt, replay[name]!)) as O;
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
  function unavailableOperationStatus() {
    return {
      serverState: "UNKNOWN",
      certainty: "outcome_unknown",
      notices: [
        {
          code: "STATUS_LOOKUP_UNAVAILABLE",
          message:
            "Server status could not be confirmed; this does not establish whether the operation committed.",
        },
      ],
    };
  }
  function operationStatusProjection(
    remote: Awaited<ReturnType<typeof original.system.operationStatus>>,
    unknownMessage: string,
  ) {
    const serverState = OperationState[remote.state] ?? "UNKNOWN";
    const operationCertainty: Record<string, string> = {
      UNKNOWN: "outcome_unknown",
      IN_PROGRESS: "in_progress",
      COMMITTED: "server_committed",
      FAILED_TERMINAL: "server_failed_terminal",
      COMPLETED: "workflow_completed_only",
    };
    const statusNotices: { code: string; message: string }[] = [];
    if (serverState === "UNKNOWN")
      statusNotices.push({
        code: "SERVER_OPERATION_UNKNOWN",
        message: unknownMessage,
      });
    if (serverState === "COMPLETED")
      statusNotices.push({
        code: "WORKFLOW_COMPLETED_ONLY",
        message:
          "Workflow execution is terminal; this does not establish domain success.",
      });
    return {
      serverState,
      serverOwner: ErrorOwner[remote.owner] ?? "UNSPECIFIED",
      ...(remote.subjectId ? { serverSubjectId: remote.subjectId } : {}),
      ...(remote.resultReference
        ? { resultReference: remote.resultReference }
        : {}),
      ...(remote.resultRevisionId
        ? { resultRevisionId: remote.resultRevisionId }
        : {}),
      ...(remote.resultEpoch !== undefined
        ? { resultEpoch: remote.resultEpoch.toString() }
        : {}),
      ...(remote.terminalCode ? { terminalCode: remote.terminalCode } : {}),
      ...(remote.recordedAt ? { recordedAt: remote.recordedAt } : {}),
      ...(remote.updatedAt ? { updatedAt: remote.updatedAt } : {}),
      ...(remote.leaseUntil ? { leaseUntil: remote.leaseUntil } : {}),
      ...(remote.committedAt ? { committedAt: remote.committedAt } : {}),
      certainty: operationCertainty[serverState] ?? "outcome_unknown",
      notices: statusNotices,
    };
  }
  async function inspectOperation(receiptId: string) {
    const receipt = await local.receipt(receiptId);
    const receiptSubjectId = receipt.subjectId ?? "";
    const statusLookup = receiptDefinition(receipt.command).statusLookup;
    if (statusLookup === "unsupported")
      return {
        receiptId,
        command: receipt.command,
        owner: receipt.owner,
        operationKind: receipt.operationKind,
        operationId: receipt.operationId,
        ...(receipt.subjectId ? { subjectId: receipt.subjectId } : {}),
        localStatus: receipt.status,
        serverState: "UNKNOWN",
        certainty: "outcome_unknown",
        notices: [
          {
            code: "STATUS_LOOKUP_UNSUPPORTED",
            message: `${receipt.owner} has no operation-status lookup for this command; a retry reuses the saved request and its stable event identity.`,
          },
        ],
      };
    let identity;
    switch (statusLookup) {
      case "subjectCreate":
        identity = {
          case: "subjectCreate" as const,
          value: create(SubjectCreateIdentitySchema, {
            subjectId: receiptSubjectId,
            operationId: receipt.operationId,
          }),
        };
        break;
      case "configurationMutation":
        identity = {
          case: "configurationMutation" as const,
          value: create(ConfigurationMutationIdentitySchema, {
            operationId: receipt.operationId,
          }),
        };
        break;
      case "observationRequest":
        identity = {
          case: "observationRequest" as const,
          value: create(ObservationRequestIdentitySchema, {
            subjectId: receiptSubjectId,
            requestId: receipt.operationId,
          }),
        };
        break;
      case "modelWorkflow": {
        const owner =
          receipt.command === "resources.materialize" ? "material" : "memory";
        identity = {
          case: "modelWorkflow" as const,
          value: create(ModelWorkflowIdentitySchema, {
            subjectId: receiptSubjectId,
            owner,
            operationKey:
              owner === "material"
                ? `resource:${receipt.operationId}`
                : receipt.operationId,
          }),
        };
        break;
      }
      case "mutationReceipt":
        identity = {
          case: "mutationReceipt" as const,
          value: create(MutationReceiptIdentitySchema, {
            subjectId: receiptSubjectId,
            operationId: receipt.operationId,
            operationKind: receipt.operationKind,
          }),
        };
        break;
    }
    let remote;
    try {
      remote = await original.system.operationStatus(
        create(GetOperationStatusRequestSchema, { identity }),
      );
    } catch (error) {
      if (error instanceof NousError) {
        return {
          receiptId,
          command: receipt.command,
          owner: receipt.owner,
          operationKind: receipt.operationKind,
          operationId: receipt.operationId,
          ...(receipt.subjectId ? { subjectId: receipt.subjectId } : {}),
          localStatus: receipt.status,
          ...unavailableOperationStatus(),
        };
      }
      throw error;
    }
    const projection = operationStatusProjection(
      remote,
      receipt.status === "complete"
        ? "No server status row was found; the local complete receipt remains unchanged."
        : "A missing server status row is not evidence that the operation never committed.",
    );
    const safeNotices = [...projection.notices];
    if (
      projection.serverState === "COMMITTED" &&
      (receipt.status === "pending" || receipt.status === "outcome_unknown")
    )
      safeNotices.push({
        code: "SERVER_COMMITTED_LOCAL_RECEIPT_PENDING",
        message:
          "The server reports committed; the local receipt is not synchronized.",
      });
    return {
      receiptId,
      command: receipt.command,
      owner: receipt.owner,
      operationKind: receipt.operationKind,
      operationId: receipt.operationId,
      ...(receipt.subjectId ? { subjectId: receipt.subjectId } : {}),
      localStatus: receipt.status,
      ...projection,
      notices: safeNotices,
    };
  }
  async function inspectOperationIdentity(request: GetOperationStatusRequest) {
    const identity = request.identity;
    const safeIdentity = (() => {
      switch (identity.case) {
        case "mutationReceipt":
          return {
            case: identity.case,
            subjectId: identity.value.subjectId,
            operationId: identity.value.operationId,
            operationKind: identity.value.operationKind,
          };
        case "subjectCreate":
          return {
            case: identity.case,
            subjectId: identity.value.subjectId,
            operationId: identity.value.operationId,
          };
        case "configurationMutation":
          return {
            case: identity.case,
            operationId: identity.value.operationId,
          };
        case "observationRequest":
          return {
            case: identity.case,
            subjectId: identity.value.subjectId,
            requestId: identity.value.requestId,
          };
        case "modelWorkflow":
          return {
            case: identity.case,
            subjectId: identity.value.subjectId,
            owner: identity.value.owner,
            operationKey: identity.value.operationKey,
          };
        case undefined:
          return { case: "unspecified" };
      }
    })();
    try {
      const remote = await original.system.operationStatus(request);
      return {
        identity: safeIdentity,
        ...operationStatusProjection(
          remote,
          "No server status row was found; a missing row does not establish whether the operation committed.",
        ),
      };
    } catch (error) {
      if (!(error instanceof NousError)) throw error;
      return {
        identity: safeIdentity,
        ...unavailableOperationStatus(),
      };
    }
  }
  async function retry(id: string) {
    const receipt = await local.receipt(id);
    if (receipt.subjectId && receipt.subjectId !== subjectId)
      throw new CliError(
        "RESULT_SUBJECT_MISMATCH",
        "Use --subject with the Subject ID frozen in this operation receipt",
      );
    if (receipt.status === "complete")
      return thawResult(receipt.command, receipt.result);
    if (receipt.status === "rejected")
      throw new CliError(
        receipt.error.code ?? "OPERATION_REJECTED",
        receipt.error.message,
        [],
        [],
        receipt.error.recovery,
        receipt.error.context,
      );
    const invoke = replay[receipt.command];
    if (!invoke)
      throw new CliError("INVALID_ARGUMENT", "Receipt command is unavailable");
    return executeReceipt(id, receipt, invoke);
  }
  return { client, retry, inspectOperation, inspectOperationIdentity };
}

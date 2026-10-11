// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { NousError, type NousClient } from "@nous-wave/client";
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
    const { request, spec, identity } = saved;
    let result: unknown;
    try {
      result = await invoke(request);
    } catch (error) {
      const rejection =
        error instanceof NousError && error.semanticComplete
          ? error
          : undefined;
      const noCommit =
        rejection?.errorCode &&
        ((rejection.owner === identity.owner &&
          spec.noCommitCodes(identity.owner).includes(rejection.errorCode)) ||
          (rejection.owner === "KERNEL" &&
            rejection.errorCode === "INVALID_REQUEST"));
      if (rejection && noCommit) {
        try {
          await local.writeReceipt(id, {
            ...receipt,
            status: "rejected",
            error: {
              transportCode: rejection.transportCode,
              code: rejection.errorCode,
              owner: rejection.owner,
              message: rejection.message.slice(0, 2048),
              ...(rejection.recovery ? { recovery: rejection.recovery } : {}),
              context: { ...rejection.context },
              ...(rejection.traceId ? { traceId: rejection.traceId } : {}),
            },
            updatedAt: new Date().toISOString(),
          });
        } catch {
          Object.assign(rejection, {
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
        throw rejection;
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
      const frozen = freezeRequest(name, request);
      const createdAt = new Date().toISOString();
      const receipt = receiptSchema.parse({
        format: "nous.consumer.operation.v2",
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
  function unavailableOperationStatus() {
    return {
      serverState: "UNKNOWN",
      serverOwner: "UNSPECIFIED",
      certainty: "outcome_unknown",
      notices: [
        {
          code: "STATUS_LOOKUP_UNAVAILABLE",
          message:
            "Server status is unavailable. Retry the status lookup or replay the saved request.",
        },
      ],
    };
  }
  async function readOperationStatus(
    request: GetOperationStatusRequest,
    unknownMessage: string,
    expectedOwner?: string,
  ) {
    try {
      return operationStatusProjection(
        await original.system.operationStatus(request),
        unknownMessage,
        expectedOwner,
      );
    } catch (error) {
      if (!(error instanceof NousError)) throw error;
      return unavailableOperationStatus();
    }
  }
  function operationStatusProjection(
    remote: Awaited<ReturnType<typeof original.system.operationStatus>>,
    unknownMessage: string,
    expectedOwner?: string,
  ) {
    const serverState = OperationState[remote.state] ?? "UNKNOWN";
    const operationCertainty: Record<string, string> = {
      UNKNOWN: "outcome_unknown",
      IN_PROGRESS: "in_progress",
      COMMITTED: "server_committed",
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
          "Workflow completed. Inspect its outcome for committed changes or a skipped result.",
      });
    const serverOwner = ErrorOwner[remote.owner] ?? "UNSPECIFIED";
    const ownerMismatch =
      expectedOwner !== undefined &&
      serverState !== "UNKNOWN" &&
      serverOwner !== expectedOwner;
    if (ownerMismatch)
      statusNotices.push({
        code: "SERVER_OWNER_MISMATCH",
        message:
          "The server owner differs from the saved operation target. Check the endpoint and receipt.",
      });
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
      certainty: ownerMismatch
        ? "outcome_unknown"
        : (operationCertainty[serverState] ?? "outcome_unknown"),
      notices: statusNotices,
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
        serverState: "UNKNOWN",
        certainty: "outcome_unknown",
        notices: [
          {
            code: "STATUS_LOOKUP_UNSUPPORTED",
            message: `${frozenIdentity.owner} has no operation-status lookup for this command; a retry reuses the saved request and its stable event identity.`,
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
                ? `resource:${frozenIdentity.operationId}`
                : frozenIdentity.operationId,
          }),
        };
        break;
      }
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
    const expectedOwner =
      receipt.command === "cognition.mutate" ? frozenIdentity.owner : undefined;
    const projection = await readOperationStatus(
      create(GetOperationStatusRequestSchema, { identity }),
      receipt.status === "complete"
        ? "No server status row was found; the local complete receipt remains unchanged."
        : "A missing server status row is not evidence that the operation never committed.",
      expectedOwner,
    );
    const safeNotices = [...projection.notices];
    if (
      projection.serverState === "COMMITTED" &&
      (!expectedOwner || projection.serverOwner === expectedOwner) &&
      receipt.status === "pending"
    )
      safeNotices.push({
        code: "SERVER_COMMITTED_LOCAL_RECEIPT_PENDING",
        message:
          "The server reports committed; the local receipt is not synchronized.",
      });
    return {
      ...receiptInfo(receipt, frozenIdentity),
      localStatus: receipt.status,
      ...projection,
      notices: safeNotices,
    };
  }
  async function inspectOperationIdentity(request: GetOperationStatusRequest) {
    return {
      identity: safeOperationStatusIdentity(request.identity),
      ...(await readOperationStatus(
        request,
        "No server status row was found; a missing row does not establish whether the operation committed.",
      )),
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
    return executeReceipt(id, receipt, invoke, saved);
  }
  return { client, retry, inspectOperation, inspectOperationIdentity };
}

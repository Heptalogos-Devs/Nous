// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { create } from "@bufbuild/protobuf";
import { CliError } from "../agent.js";
import type { CliEnvironment } from "../runtime.js";
import {
  ConfigurationMutationIdentitySchema,
  GetOperationStatusRequestSchema,
  ModelWorkflowIdentitySchema,
  MutationReceiptIdentitySchema,
  ObservationRequestIdentitySchema,
  SubjectCreateIdentitySchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/management_pb.js";

async function explicitIdentity(
  env: CliEnvironment,
  kind: string,
  args: string[],
) {
  const value = (index: number, label: string) =>
    env.required(args[index], label);
  let identity;
  if (kind === "mutation") {
    const subjectId = (
      await env.resolveReference(value(2, "Subject reference"), "subject")
    ).value;
    identity = {
      case: "mutationReceipt" as const,
      value: create(MutationReceiptIdentitySchema, {
        subjectId,
        operationId: value(3, "Operation ID"),
        operationKind: value(1, "Operation kind"),
      }),
    };
  } else if (kind === "subject-create") {
    const subjectId = (
      await env.resolveReference(value(1, "Subject reference"), "subject")
    ).value;
    identity = {
      case: "subjectCreate" as const,
      value: create(SubjectCreateIdentitySchema, {
        subjectId,
        operationId: value(2, "Operation ID"),
      }),
    };
  } else if (kind === "configuration") {
    identity = {
      case: "configurationMutation" as const,
      value: create(ConfigurationMutationIdentitySchema, {
        operationId: value(1, "Operation ID"),
      }),
    };
  } else if (kind === "observation") {
    const subjectId = (
      await env.resolveReference(value(1, "Subject reference"), "subject")
    ).value;
    identity = {
      case: "observationRequest" as const,
      value: create(ObservationRequestIdentitySchema, {
        subjectId,
        requestId: value(2, "Request ID"),
      }),
    };
  } else if (kind === "workflow") {
    const subjectId = (
      await env.resolveReference(value(1, "Subject reference"), "subject")
    ).value;
    identity = {
      case: "modelWorkflow" as const,
      value: create(ModelWorkflowIdentitySchema, {
        subjectId,
        owner: value(2, "Workflow owner"),
        operationKey: value(3, "Operation key"),
      }),
    };
  } else {
    throw new CliError(
      "INVALID_ARGUMENT",
      `Unknown operation identity: ${kind}`,
    );
  }
  return env.inspectOperationIdentity(
    create(GetOperationStatusRequestSchema, { identity }),
  );
}

export async function operationCommands(
  env: CliEnvironment,
  action?: string,
  argument?: string,
  positionals: string[] = [],
) {
  if (action === "inspect" && argument) {
    const args = positionals.slice(2);
    if (
      [
        "mutation",
        "subject-create",
        "configuration",
        "observation",
        "workflow",
      ].includes(args[0] ?? "")
    )
      return explicitIdentity(env, args[0]!, args);
    return env.inspectOperation(env.required(argument, "Saved receipt ID"));
  }
  throw new CliError(
    "INVALID_ARGUMENT",
    "Use operation inspect <receipt-id|typed identity>",
  );
}

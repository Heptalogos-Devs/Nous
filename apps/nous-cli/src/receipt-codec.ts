// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import {
  create,
  fromJson,
  isMessage,
  toJson,
  type DescMessage,
  type DescMethod,
  type JsonValue,
} from "@bufbuild/protobuf";
import { ConfigurationService } from "@nous-wave/protocol/nous/wave/v1alpha1/configuration_pb.js";
import {
  ConceptService,
  ConfigurationMutationIdentitySchema,
  ModelWorkflowIdentitySchema,
  MutationReceiptIdentitySchema,
  ObservationRequestIdentitySchema,
  ResourceService,
  SubjectCreateIdentitySchema,
  type GetOperationStatusRequest,
} from "@nous-wave/protocol/nous/wave/v1alpha1/management_pb.js";
import {
  MemoryService,
  RuntimeService,
  SubjectService,
} from "@nous-wave/protocol/nous/wave/v1alpha1/services_pb.js";
import { ModelService } from "@nous-wave/protocol/nous/wave/v1alpha1/model_pb.js";
import type { NousClient } from "@nous-wave/client";

type ReceiptStatusLookup =
  | "subjectCreate"
  | "configurationMutation"
  | "observationRequest"
  | "modelWorkflow"
  | "mutationReceipt"
  | "unsupported";

type ReceiptOwner = string | ((input: Record<string, unknown>) => string);
type ReceiptRegistration = readonly [
  owner: ReceiptOwner,
  operationKind: string,
  method: DescMethod,
];
interface ReceiptDefinition {
  owner(input: Record<string, unknown>): string;
  operationKind: string;
  request: DescMessage;
  response: DescMessage;
  statusLookup: ReceiptStatusLookup;
  noCommitCodes(owner: string): readonly string[];
}

const cognitionNoCommitCodes = [
  "INVALID_REQUEST",
  "STALE_REVISION",
  "OPERATION_ID_CONFLICT",
  "UNKNOWN_REFERENCE",
  "REFERENCE_TYPE_MISMATCH",
  "REFERENCE_TOMBSTONED",
] as const;
const ownerNoCommitCodes: Record<string, readonly string[]> = {
  SUBJECT: ["INVALID_REQUEST", "OPERATION_ID_CONFLICT"],
  CONFIGURATION: ["INVALID_REQUEST", "STALE_REVISION", "OPERATION_ID_CONFLICT"],
  OBSERVATION: [
    "INVALID_REQUEST",
    "OPERATION_ID_CONFLICT",
    "UNKNOWN_REFERENCE",
  ],
  MODEL_WORKFLOW: [],
  RUNTIME: ["INVALID_REQUEST", "STALE_CONTEXT", "OPERATION_ID_CONFLICT"],
  CONCEPT: cognitionNoCommitCodes,
  MEMORY: cognitionNoCommitCodes,
};

function cognitionMutationOwner(input: Record<string, unknown>) {
  const target = input.objectRef;
  const kind =
    target && typeof target === "object"
      ? (target as Record<string, unknown>).kind
      : undefined;
  if (kind === "cognitive_schema" || kind === "cognitive_schema_revision")
    return "CONCEPT";
  if (
    [
      "memory",
      "memory_revision",
      "episode",
      "episode_revision",
      "journal",
      "journal_revision",
    ].includes(String(kind))
  )
    return "MEMORY";
  throw new Error("Cognition mutation receipt has an unsupported target kind");
}

/** Current receipt semantics paired with the generated RPC's canonical message schemas. */
const receiptRegistrations = {
  "subjects.create": [
    "SUBJECT",
    "subject_create",
    SubjectService.method.createSubject,
  ],
  "subjects.adoptSeed": [
    "SUBJECT",
    "cognitive_seed_adopt",
    SubjectService.method.adoptCognitiveSeed,
  ],
  "configuration.setSystem": [
    "CONFIGURATION",
    "configuration_mutation",
    ConfigurationService.method.setSystemOverride,
  ],
  "configuration.setSubject": [
    "CONFIGURATION",
    "configuration_mutation",
    ConfigurationService.method.setSubjectOverride,
  ],
  "configuration.clearSystem": [
    "CONFIGURATION",
    "configuration_mutation",
    ConfigurationService.method.clearSystemOverride,
  ],
  "configuration.clearSubject": [
    "CONFIGURATION",
    "configuration_mutation",
    ConfigurationService.method.clearSubjectOverride,
  ],
  "model.formFromObservation": [
    "MODEL_WORKFLOW",
    "form_memory",
    ModelService.method.formFromObservation,
  ],
  "cognition.observe": [
    "OBSERVATION",
    "observation_request",
    RuntimeService.method.recordObservation,
  ],
  "cognition.reportUse": [
    "RUNTIME",
    "report_use",
    RuntimeService.method.reportUse,
  ],
  "cognition.createWorkContext": [
    "RUNTIME",
    "create_work_context",
    RuntimeService.method.createWorkContext,
  ],
  "cognition.updateWorkContext": [
    "RUNTIME",
    "update_work_context",
    RuntimeService.method.updateWorkContext,
  ],
  "cognition.pauseWorkContext": [
    "RUNTIME",
    "pause_work_context",
    RuntimeService.method.pauseWorkContext,
  ],
  "cognition.resumeWorkContext": [
    "RUNTIME",
    "resume_work_context",
    RuntimeService.method.resumeWorkContext,
  ],
  "cognition.endWorkContext": [
    "RUNTIME",
    "end_work_context",
    RuntimeService.method.endWorkContext,
  ],
  "cognition.setActiveWorkContext": [
    "RUNTIME",
    "set_active_work_context",
    RuntimeService.method.setActiveWorkContext,
  ],
  "cognition.mutate": [
    cognitionMutationOwner,
    "cognition_lifecycle",
    MemoryService.method.mutateCognition,
  ],
  "concepts.createTag": [
    "CONCEPT",
    "create_tag",
    ConceptService.method.createTag,
  ],
  "concepts.reviseTag": [
    "CONCEPT",
    "revise_tag",
    ConceptService.method.reviseTag,
  ],
  "concepts.mergeTags": [
    "CONCEPT",
    "merge_tags",
    ConceptService.method.mergeTags,
  ],
  "concepts.splitTag": ["CONCEPT", "split_tag", ConceptService.method.splitTag],
  "concepts.associate": [
    "CONCEPT",
    "create_association",
    ConceptService.method.createAssociation,
  ],
  "concepts.revokeAssociation": [
    "CONCEPT",
    "revoke_association",
    ConceptService.method.revokeAssociation,
  ],
  "memory.revise": [
    "MEMORY",
    "revise_memory",
    MemoryService.method.reviseMemory,
  ],
  "memory.linkRevisions": [
    "MEMORY",
    "link_memory_revisions",
    MemoryService.method.linkRevisions,
  ],
  "memory.createEpisode": [
    "MEMORY",
    "create_episode",
    MemoryService.method.createEpisode,
  ],
  "memory.reviseEpisode": [
    "MEMORY",
    "revise_episode",
    MemoryService.method.reviseEpisode,
  ],
  "memory.linkEpisodeRevisions": [
    "MEMORY",
    "link_episode_revisions",
    MemoryService.method.linkEpisodeRevisions,
  ],
  "concepts.createSchema": [
    "CONCEPT",
    "create_cognitive_schema",
    ConceptService.method.createCognitiveSchema,
  ],
  "concepts.addSchemaEvidence": [
    "CONCEPT",
    "add_schema_evidence",
    ConceptService.method.addSchemaEvidence,
  ],
  "concepts.reviseSchema": [
    "CONCEPT",
    "revise_cognitive_schema",
    ConceptService.method.reviseCognitiveSchema,
  ],
  "concepts.splitSchema": [
    "CONCEPT",
    "split_cognitive_schema",
    ConceptService.method.splitCognitiveSchema,
  ],
  "concepts.mergeSchemas": [
    "CONCEPT",
    "merge_cognitive_schemas",
    ConceptService.method.mergeCognitiveSchemas,
  ],
  "resources.materialize": [
    "MODEL_WORKFLOW",
    "resource_materialization",
    ResourceService.method.materializeResource,
  ],
} as const satisfies Partial<
  Record<ReceiptClientMethodPath, ReceiptRegistration>
>;

const operationIdentitySchemas = {
  mutationReceipt: MutationReceiptIdentitySchema,
  subjectCreate: SubjectCreateIdentitySchema,
  configurationMutation: ConfigurationMutationIdentitySchema,
  observationRequest: ObservationRequestIdentitySchema,
  modelWorkflow: ModelWorkflowIdentitySchema,
} as const;

export function safeOperationStatusIdentity(
  identity: GetOperationStatusRequest["identity"],
) {
  if (!identity.case) return { case: "unspecified" };
  const fields = operationIdentitySchemas[identity.case].fields;
  return {
    case: identity.case,
    ...Object.fromEntries(
      fields.map(({ localName }) => [
        localName,
        (identity.value as Record<string, unknown>)[localName],
      ]),
    ),
  };
}

type ReceiptClientMethodPath = {
  [Namespace in keyof NousClient]: `${Namespace}.${Extract<
    keyof NousClient[Namespace],
    string
  >}`;
}[keyof NousClient];

type ReceiptCommand = keyof typeof receiptRegistrations;

function statusLookup(
  command: ReceiptCommand,
  owner: string,
): ReceiptStatusLookup {
  if (command === "subjects.create") return "subjectCreate";
  if (command === "subjects.adoptSeed" || command === "cognition.reportUse")
    return "unsupported";
  if (owner === "CONFIGURATION") return "configurationMutation";
  if (owner === "OBSERVATION") return "observationRequest";
  if (owner === "MODEL_WORKFLOW") return "modelWorkflow";
  return "mutationReceipt";
}

export function receiptDefinitionCommands(): readonly ReceiptCommand[] {
  return Object.keys(receiptRegistrations) as ReceiptCommand[];
}

export function receiptDefinition(command: string): ReceiptDefinition {
  const found = receiptRegistrations[command as ReceiptCommand] as
    ReceiptRegistration | undefined;
  if (!found) throw new Error(`Receipt command is not registered: ${command}`);
  const [registeredOwner, operationKind, method] = found;
  const owner = (input: Record<string, unknown>) =>
    typeof registeredOwner === "string"
      ? registeredOwner
      : registeredOwner(input);
  const resolvedOwner =
    typeof registeredOwner === "string" ? registeredOwner : "";
  const definition: ReceiptDefinition = {
    owner,
    operationKind,
    request: method.input,
    response: method.output,
    statusLookup: statusLookup(command as ReceiptCommand, resolvedOwner),
    noCommitCodes: (ownerName) => ownerNoCommitCodes[ownerName] ?? [],
  };
  return definition;
}

function operationId(
  command: string,
  requestSchema: DescMessage,
  input: Record<string, unknown>,
) {
  let value: unknown;
  if (command === "cognition.reportUse") {
    const events = input.events;
    value =
      Array.isArray(events) && events[0] && typeof events[0] === "object"
        ? (events[0] as Record<string, unknown>).eventId
        : undefined;
  } else {
    const field =
      requestSchema.field.operationId ??
      requestSchema.field.requestId ??
      requestSchema.field.eventId;
    value = field ? input[field.localName] : undefined;
  }
  if (typeof value !== "string" || !value)
    throw new Error("Mutation request needs a stable operation identity");
  return value;
}

function freezeMessage(schema: DescMessage, value: unknown, error: string) {
  if (!isMessage(value, schema)) throw new Error(error);
  return {
    typeName: schema.typeName,
    protoJson: toJson(schema, value as never),
  };
}

function thawMessage(
  schema: DescMessage,
  typeName: string,
  protoJson: unknown,
  error: string,
) {
  if (typeName !== schema.typeName) throw new Error(error);
  return fromJson(schema, protoJson as JsonValue);
}

export function freezeRequest(command: string, input: unknown) {
  const spec = receiptDefinition(command);
  if (!input || typeof input !== "object")
    throw new Error("Mutation request must be an object");
  const request = create(spec.request as never, input as never);
  const frozen = freezeMessage(
    spec.request,
    request,
    "Mutation request did not match its registered protobuf schema",
  );
  const identity = requestIdentity(
    command,
    spec,
    request as unknown as Record<string, unknown>,
  );
  return { frozen, request, spec, identity };
}

function requestIdentity(
  command: string,
  spec: ReceiptDefinition,
  record: Record<string, unknown>,
) {
  const subjectId = record.subjectId;
  if (
    spec.request.field.subjectId &&
    (typeof subjectId !== "string" || !subjectId)
  )
    throw new Error("Subject-scoped mutation requires a Subject ID");
  return {
    owner: spec.owner(record),
    operationKind: spec.operationKind,
    operationId: operationId(command, spec.request, record),
    subjectId:
      typeof subjectId === "string" && subjectId ? subjectId : undefined,
  };
}

export function receiptIdentity(command: string, request: unknown) {
  const spec = receiptDefinition(command);
  if (!request || typeof request !== "object")
    throw new Error("Mutation request must be an object");
  return requestIdentity(command, spec, request as Record<string, unknown>);
}

export function thawRequest(
  command: string,
  typeName: string,
  protoJson: unknown,
) {
  const spec = receiptDefinition(command);
  return thawMessage(
    spec.request,
    typeName,
    protoJson,
    "Receipt request type does not match its command registry",
  );
}

export function freezeResult(command: string, result: unknown) {
  return freezeMessage(
    receiptDefinition(command).response,
    result,
    "Mutation response did not match its registered protobuf schema",
  );
}

export function thawResult(
  command: string,
  value: { typeName: string; protoJson: unknown },
) {
  return thawMessage(
    receiptDefinition(command).response,
    value.typeName,
    value.protoJson,
    "Receipt result type does not match its command registry",
  );
}

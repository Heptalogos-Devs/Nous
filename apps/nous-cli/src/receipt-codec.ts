// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import {
  create,
  fromJson,
  isMessage,
  toJson,
  type DescMessage,
  type JsonValue,
} from "@bufbuild/protobuf";
import { EmptySchema, ValueSchema } from "@bufbuild/protobuf/wkt";
import {
  ClearSubjectOverrideRequestSchema,
  ClearSystemOverrideRequestSchema,
  ConfigurationChangeSchema,
  SetSubjectOverrideRequestSchema,
  SetSystemOverrideRequestSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/configuration_pb.js";
import {
  AcceptedObservationSchema,
  CreateSubjectRequestSchema,
  AdoptCognitiveSeedRequestSchema,
  CognitiveSeedVersionSchema,
  MemorySchema,
  MemoryMutationRequestSchema,
  LinkRevisionsRequestSchema,
  SetAccessibilityRequestSchema,
  ReportUseRequestSchema,
  ReportUseResponseSchema,
  ReviseMemoryRequestSchema,
  SetActiveWorkContextRequestSchema,
  SessionSchema,
  SubjectSchema,
  WorkContextResponseSchema,
  CreateWorkContextRequestSchema,
  UpdateWorkContextRequestSchema,
  WorkContextMutationRequestSchema,
  ObservationInputSchema,
  CreateEpisodeRequestSchema,
  ReviseEpisodeRequestSchema,
  LinkEpisodeRevisionsRequestSchema,
  EpisodeMutationRequestSchema,
  EpisodeResponseSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import {
  AddSchemaEvidenceRequestSchema,
  CognitiveSchemaMutationRequestSchema,
  CognitiveSchemaSchema,
  CreateCognitiveSchemaRequestSchema,
  MergeCognitiveSchemasRequestSchema,
  ReviseCognitiveSchemaRequestSchema,
  SplitCognitiveSchemaRequestSchema,
  SplitCognitiveSchemaResponseSchema,
  MaterializeResourceRequestSchema,
  ResourceMaterializationSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/management_pb.js";
import {
  JournalMutationRequestSchema,
  JournalResponseSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/journal_pb.js";
import {
  AssociationSchema,
  CreateAssociationRequestSchema,
  CreateTagRequestSchema,
  MergeTagsRequestSchema,
  RevokeAssociationRequestSchema,
  ReviseTagRequestSchema,
  SplitTagRequestSchema,
  SplitTagResponseSchema,
  TagSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/management_pb.js";
import {
  FormationRequestSchema,
  FormationResponseSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/model_pb.js";

export type ReceiptStatusLookup =
  | "subjectCreate"
  | "configurationMutation"
  | "observationRequest"
  | "modelWorkflow"
  | "mutationReceipt"
  | "unsupported";

interface ReceiptDefinition {
  owner: string;
  operationKind: string;
  request: DescMessage;
  response: DescMessage;
  subjectScope: "required" | "optional";
  operationId(input: Record<string, unknown>): string;
  statusLookup: ReceiptStatusLookup;
  noCommitCodes: readonly string[];
  prepare?(input: Record<string, unknown>): unknown;
}
type ReceiptDefinitionOptions = {
  subjectScope?: ReceiptDefinition["subjectScope"];
  operationId?: ReceiptDefinition["operationId"];
  statusLookup?: ReceiptStatusLookup;
  prepare?: ReceiptDefinition["prepare"];
};
const operationField = (input: Record<string, unknown>) => {
  const id = input.operationId ?? input.requestId ?? input.eventId;
  return typeof id === "string" && id ? id : "";
};
const requiredOperationId = (input: Record<string, unknown>) => {
  const id = operationField(input);
  if (!id) throw new Error("Mutation request needs an operation identity");
  return id;
};
const reportUseOperationId = (input: Record<string, unknown>) => {
  const events = input.events;
  const id =
    Array.isArray(events) && events[0] && typeof events[0] === "object"
      ? (events[0] as Record<string, unknown>).eventId
      : undefined;
  if (typeof id !== "string" || !id)
    throw new Error("UseEvent request needs its stable event identity");
  return id;
};
const definition = (
  owner: string,
  operationKind: string,
  request: DescMessage,
  response: DescMessage,
  options: ReceiptDefinitionOptions = {},
): ReceiptDefinition => ({
  owner,
  operationKind,
  request,
  response,
  subjectScope: options.subjectScope ?? "required",
  operationId: options.operationId ?? requiredOperationId,
  statusLookup: options.statusLookup ?? "mutationReceipt",
  noCommitCodes:
    {
      SUBJECT: ["INVALID_REQUEST", "OPERATION_ID_CONFLICT"],
      CONFIGURATION: [
        "INVALID_REQUEST",
        "STALE_REVISION",
        "OPERATION_ID_CONFLICT",
      ],
      OBSERVATION: [
        "INVALID_REQUEST",
        "OPERATION_ID_CONFLICT",
        "UNKNOWN_REFERENCE",
      ],
      MODEL_WORKFLOW: [],
      RUNTIME: ["INVALID_REQUEST", "STALE_CONTEXT", "OPERATION_ID_CONFLICT"],
      CONCEPT: [
        "INVALID_REQUEST",
        "STALE_REVISION",
        "OPERATION_ID_CONFLICT",
        "UNKNOWN_REFERENCE",
        "REFERENCE_TYPE_MISMATCH",
        "REFERENCE_TOMBSTONED",
      ],
      MEMORY: [
        "INVALID_REQUEST",
        "STALE_REVISION",
        "OPERATION_ID_CONFLICT",
        "UNKNOWN_REFERENCE",
        "REFERENCE_TYPE_MISMATCH",
        "REFERENCE_TOMBSTONED",
      ],
    }[owner] ?? [],
  prepare: options.prepare,
});

/** Static request/response and status identities share one receipt registry. */
const receiptDefinitions = {
  "subjects.create": definition(
    "SUBJECT",
    "subject_create",
    CreateSubjectRequestSchema,
    SubjectSchema,
    { statusLookup: "subjectCreate" },
  ),
  "subjects.adoptSeed": definition(
    "SUBJECT",
    "cognitive_seed_adopt",
    AdoptCognitiveSeedRequestSchema,
    CognitiveSeedVersionSchema,
    { statusLookup: "unsupported" },
  ),
  "configuration.setSystem": definition(
    "CONFIGURATION",
    "configuration_mutation",
    SetSystemOverrideRequestSchema,
    ConfigurationChangeSchema,
    {
      subjectScope: "optional",
      prepare: (input) =>
        create(SetSystemOverrideRequestSchema, {
          ...input,
          value: fromJson(ValueSchema, input.value as JsonValue),
        }),
      statusLookup: "configurationMutation",
    },
  ),
  "configuration.setSubject": definition(
    "CONFIGURATION",
    "configuration_mutation",
    SetSubjectOverrideRequestSchema,
    ConfigurationChangeSchema,
    {
      prepare: (input) =>
        create(SetSubjectOverrideRequestSchema, {
          ...input,
          value: fromJson(ValueSchema, input.value as JsonValue),
        }),
      statusLookup: "configurationMutation",
    },
  ),
  "configuration.clearSystem": definition(
    "CONFIGURATION",
    "configuration_mutation",
    ClearSystemOverrideRequestSchema,
    ConfigurationChangeSchema,
    { subjectScope: "optional", statusLookup: "configurationMutation" },
  ),
  "configuration.clearSubject": definition(
    "CONFIGURATION",
    "configuration_mutation",
    ClearSubjectOverrideRequestSchema,
    ConfigurationChangeSchema,
    { statusLookup: "configurationMutation" },
  ),
  "model.formFromObservation": definition(
    "MODEL_WORKFLOW",
    "form_memory",
    FormationRequestSchema,
    FormationResponseSchema,
    { statusLookup: "modelWorkflow" },
  ),
  "cognition.observe": definition(
    "OBSERVATION",
    "observation_request",
    ObservationInputSchema,
    AcceptedObservationSchema,
    { statusLookup: "observationRequest" },
  ),
  "cognition.reportUse": definition(
    "RUNTIME",
    "report_use",
    ReportUseRequestSchema,
    ReportUseResponseSchema,
    { operationId: reportUseOperationId, statusLookup: "unsupported" },
  ),
  "cognition.createWorkContext": definition(
    "RUNTIME",
    "create_work_context",
    CreateWorkContextRequestSchema,
    WorkContextResponseSchema,
  ),
  "cognition.updateWorkContext": definition(
    "RUNTIME",
    "update_work_context",
    UpdateWorkContextRequestSchema,
    WorkContextResponseSchema,
  ),
  "cognition.pauseWorkContext": definition(
    "RUNTIME",
    "pause_work_context",
    WorkContextMutationRequestSchema,
    WorkContextResponseSchema,
  ),
  "cognition.resumeWorkContext": definition(
    "RUNTIME",
    "resume_work_context",
    WorkContextMutationRequestSchema,
    WorkContextResponseSchema,
  ),
  "cognition.endWorkContext": definition(
    "RUNTIME",
    "end_work_context",
    WorkContextMutationRequestSchema,
    WorkContextResponseSchema,
  ),
  "cognition.setActiveWorkContext": definition(
    "RUNTIME",
    "set_active_work_context",
    SetActiveWorkContextRequestSchema,
    SessionSchema,
  ),
  "concepts.createTag": definition(
    "CONCEPT",
    "create_tag",
    CreateTagRequestSchema,
    TagSchema,
  ),
  "concepts.reviseTag": definition(
    "CONCEPT",
    "revise_tag",
    ReviseTagRequestSchema,
    TagSchema,
  ),
  "concepts.mergeTags": definition(
    "CONCEPT",
    "merge_tags",
    MergeTagsRequestSchema,
    TagSchema,
  ),
  "concepts.splitTag": definition(
    "CONCEPT",
    "split_tag",
    SplitTagRequestSchema,
    SplitTagResponseSchema,
  ),
  "concepts.associate": definition(
    "CONCEPT",
    "create_association",
    CreateAssociationRequestSchema,
    AssociationSchema,
  ),
  "concepts.revokeAssociation": definition(
    "CONCEPT",
    "revoke_association",
    RevokeAssociationRequestSchema,
    EmptySchema,
  ),
  "memory.revise": definition(
    "MEMORY",
    "revise_memory",
    ReviseMemoryRequestSchema,
    MemorySchema,
  ),
  "memory.setAccessibility": definition(
    "MEMORY",
    "set_accessibility",
    SetAccessibilityRequestSchema,
    MemorySchema,
  ),
  "memory.linkRevisions": definition(
    "MEMORY",
    "link_memory_revisions",
    LinkRevisionsRequestSchema,
    EmptySchema,
  ),
  "memory.suppress": definition(
    "MEMORY",
    "suppression_state",
    MemoryMutationRequestSchema,
    MemorySchema,
  ),
  "memory.restore": definition(
    "MEMORY",
    "suppression_state",
    MemoryMutationRequestSchema,
    MemorySchema,
  ),
  "memory.withdraw": definition(
    "MEMORY",
    "acceptance_state",
    MemoryMutationRequestSchema,
    MemorySchema,
  ),
  "memory.reaccept": definition(
    "MEMORY",
    "acceptance_state",
    MemoryMutationRequestSchema,
    MemorySchema,
  ),
  "memory.purge": definition(
    "MEMORY",
    "purge_memory",
    MemoryMutationRequestSchema,
    EmptySchema,
  ),
  "memory.createEpisode": definition(
    "MEMORY",
    "create_episode",
    CreateEpisodeRequestSchema,
    EpisodeResponseSchema,
  ),
  "memory.reviseEpisode": definition(
    "MEMORY",
    "revise_episode",
    ReviseEpisodeRequestSchema,
    EpisodeResponseSchema,
  ),
  "memory.linkEpisodeRevisions": definition(
    "MEMORY",
    "link_episode_revisions",
    LinkEpisodeRevisionsRequestSchema,
    EmptySchema,
  ),
  "memory.suppressEpisode": definition(
    "MEMORY",
    "episode_lifecycle",
    EpisodeMutationRequestSchema,
    EpisodeResponseSchema,
  ),
  "memory.restoreEpisode": definition(
    "MEMORY",
    "episode_lifecycle",
    EpisodeMutationRequestSchema,
    EpisodeResponseSchema,
  ),
  "memory.withdrawEpisode": definition(
    "MEMORY",
    "episode_lifecycle",
    EpisodeMutationRequestSchema,
    EpisodeResponseSchema,
  ),
  "memory.reacceptEpisode": definition(
    "MEMORY",
    "episode_lifecycle",
    EpisodeMutationRequestSchema,
    EpisodeResponseSchema,
  ),
  "memory.purgeEpisode": definition(
    "MEMORY",
    "purge_episode",
    EpisodeMutationRequestSchema,
    EmptySchema,
  ),
  "memory.suppressJournal": definition(
    "MEMORY",
    "journal_lifecycle",
    JournalMutationRequestSchema,
    JournalResponseSchema,
  ),
  "memory.restoreJournal": definition(
    "MEMORY",
    "journal_lifecycle",
    JournalMutationRequestSchema,
    JournalResponseSchema,
  ),
  "memory.withdrawJournal": definition(
    "MEMORY",
    "journal_lifecycle",
    JournalMutationRequestSchema,
    JournalResponseSchema,
  ),
  "memory.reacceptJournal": definition(
    "MEMORY",
    "journal_lifecycle",
    JournalMutationRequestSchema,
    JournalResponseSchema,
  ),
  "memory.purgeJournal": definition(
    "MEMORY",
    "journal_purge",
    JournalMutationRequestSchema,
    EmptySchema,
  ),
  "concepts.createSchema": definition(
    "CONCEPT",
    "create_cognitive_schema",
    CreateCognitiveSchemaRequestSchema,
    CognitiveSchemaSchema,
  ),
  "concepts.addSchemaEvidence": definition(
    "CONCEPT",
    "add_schema_evidence",
    AddSchemaEvidenceRequestSchema,
    CognitiveSchemaSchema,
  ),
  "concepts.reviseSchema": definition(
    "CONCEPT",
    "revise_cognitive_schema",
    ReviseCognitiveSchemaRequestSchema,
    CognitiveSchemaSchema,
  ),
  "concepts.splitSchema": definition(
    "CONCEPT",
    "split_cognitive_schema",
    SplitCognitiveSchemaRequestSchema,
    SplitCognitiveSchemaResponseSchema,
  ),
  "concepts.mergeSchemas": definition(
    "CONCEPT",
    "merge_cognitive_schemas",
    MergeCognitiveSchemasRequestSchema,
    CognitiveSchemaSchema,
  ),
  "concepts.suppressSchema": definition(
    "CONCEPT",
    "schema_lifecycle",
    CognitiveSchemaMutationRequestSchema,
    CognitiveSchemaSchema,
  ),
  "concepts.restoreSchema": definition(
    "CONCEPT",
    "schema_lifecycle",
    CognitiveSchemaMutationRequestSchema,
    CognitiveSchemaSchema,
  ),
  "concepts.withdrawSchema": definition(
    "CONCEPT",
    "schema_lifecycle",
    CognitiveSchemaMutationRequestSchema,
    CognitiveSchemaSchema,
  ),
  "concepts.reacceptSchema": definition(
    "CONCEPT",
    "schema_lifecycle",
    CognitiveSchemaMutationRequestSchema,
    CognitiveSchemaSchema,
  ),
  "concepts.purgeSchema": definition(
    "CONCEPT",
    "schema_purge",
    CognitiveSchemaMutationRequestSchema,
    EmptySchema,
  ),
  "resources.materialize": definition(
    "MODEL_WORKFLOW",
    "resource_materialization",
    MaterializeResourceRequestSchema,
    ResourceMaterializationSchema,
    { statusLookup: "modelWorkflow" },
  ),
} satisfies Record<string, ReceiptDefinition>;

export function receiptDefinition(command: string) {
  const found = receiptDefinitions[command as keyof typeof receiptDefinitions];
  if (!found) throw new Error(`Receipt command is not registered: ${command}`);
  return found;
}

export function receiptStatusLookup(command: string): ReceiptStatusLookup {
  const found = receiptDefinitions[command as keyof typeof receiptDefinitions];
  if (!found)
    throw new Error(`Receipt status lookup is not registered: ${command}`);
  return found.statusLookup;
}

export function freezeRequest(command: string, input: unknown) {
  const spec = receiptDefinition(command);
  if (!input || typeof input !== "object")
    throw new Error("Mutation request must be an object");
  const record = input as Record<string, unknown>;
  const request = spec.prepare
    ? spec.prepare(record)
    : create(spec.request as never, record as never);
  if (!isMessage(request, spec.request))
    throw new Error(
      "Mutation request did not match its registered protobuf schema",
    );
  const requestRecord = request as unknown as Record<string, unknown>;
  const subjectId = requestRecord.subjectId;
  if (
    spec.subjectScope === "required" &&
    (typeof subjectId !== "string" || !subjectId)
  )
    throw new Error("Subject-scoped mutation requires a Subject ID");
  const operationId = spec.operationId(requestRecord);
  return {
    request,
    operationId,
    subjectId:
      typeof subjectId === "string" && subjectId ? subjectId : undefined,
    frozen: {
      typeName: spec.request.typeName,
      protoJson: toJson(spec.request, request as never),
    },
  };
}

export function thawRequest(
  command: string,
  typeName: string,
  protoJson: unknown,
) {
  const spec = receiptDefinition(command);
  if (typeName !== spec.request.typeName)
    throw new Error("Receipt request type does not match its command registry");
  return fromJson(spec.request as never, protoJson as JsonValue);
}

export function freezeResult(command: string, result: unknown) {
  const spec = receiptDefinition(command);
  if (!isMessage(result, spec.response))
    throw new Error(
      "Mutation response did not match its registered protobuf schema",
    );
  return {
    typeName: spec.response.typeName,
    protoJson: toJson(spec.response, result as never),
  };
}

export function thawResult(
  command: string,
  value: { typeName: string; protoJson: unknown },
) {
  const spec = receiptDefinition(command);
  if (value.typeName !== spec.response.typeName)
    throw new Error("Receipt result type does not match its command registry");
  return fromJson(spec.response as never, value.protoJson as JsonValue);
}

// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { fromJson, toJson, type JsonValue } from "@bufbuild/protobuf";
import { ValueSchema } from "@bufbuild/protobuf/wkt";
import { protocolData as plain, type Data } from "./data.js";
import { NousError } from "./errors.js";
export { NousError, DomainErrorCode, ErrorRecovery } from "./errors.js";
import {
  executionOpportunitySchema,
  executionEnvelope,
  executionOpportunityPaths,
} from "./policy/execution.js";
export {
  ConfigExposure,
  ConfigurationView,
} from "@nous-wave/protocol/nous/wave/v1alpha1/configuration_pb.js";
import {
  ConfigurationService,
  ConfigurationView,
  ConfigExposure,
} from "@nous-wave/protocol/nous/wave/v1alpha1/configuration_pb.js";
import {
  createClient,
  ConnectError,
  type CallOptions,
  type Transport,
} from "@connectrpc/connect";
import {
  SubjectService,
  CognitionService,
  RuntimeService,
  MemoryService,
  MaterialService,
} from "@nous-wave/protocol/nous/wave/v1alpha1/services_pb.js";
import { IdentityService } from "@nous-wave/protocol/nous/wave/v1alpha1/identity_pb.js";

/** A consumer-owned web identity, with the original public locator preserved. */
export function webSource(value: string) {
  const url = new URL(value);
  if (
    !["http:", "https:"].includes(url.protocol) ||
    url.username ||
    url.password
  )
    throw new Error("Source URL must be credential-free HTTP(S)");
  const source = url.toString();
  return {
    sourceClass: "web",
    externalObjectRef: `object:web:${source}`,
    context: { source_url: source },
  };
}
import {
  ResourceService,
  ResourceRegistryService,
  ConceptService,
  SystemService,
} from "@nous-wave/protocol/nous/wave/v1alpha1/management_pb.js";
import { ModelService } from "@nous-wave/protocol/nous/wave/v1alpha1/model_pb.js";

export interface RequestOptions {
  signal?: AbortSignal;
  timeoutMs?: number;
}
function call<I, O>(
  method: (input: I, options?: CallOptions) => Promise<O>,
  defaultTimeout?:
    number | ((input: I, options?: RequestOptions) => number | Promise<number>),
) {
  return async (input: I, options?: RequestOptions): Promise<Data<O>> => {
    try {
      const timeoutMs =
        options?.timeoutMs ??
        (typeof defaultTimeout === "function"
          ? await defaultTimeout(input, options)
          : defaultTimeout);
      return plain(
        await method(
          input,
          timeoutMs === undefined ? options : { ...options, timeoutMs },
        ),
      );
    } catch (error) {
      if (error instanceof ConnectError) throw new NousError(error);
      throw error;
    }
  };
}
export interface ConfigurationMutation {
  operationId: string;
  path: string;
  expectedRevision: bigint;
}
export interface ConfigurationOverride extends ConfigurationMutation {
  value: JsonValue;
}

export function createNousClient(transport: Transport) {
  const subjects = createClient(SubjectService, transport);
  const cognition = createClient(CognitionService, transport);
  const runtime = createClient(RuntimeService, transport);
  const memory = createClient(MemoryService, transport);
  const material = createClient(MaterialService, transport);
  const identity = createClient(IdentityService, transport);
  const resources = createClient(ResourceService, transport);
  const registry = createClient(ResourceRegistryService, transport);
  const concepts = createClient(ConceptService, transport);
  const system = createClient(SystemService, transport);
  const model = createClient(ModelService, transport);
  const configuration = createClient(ConfigurationService, transport);
  const executionTimeout = async (
    workMs?: number,
    options?: RequestOptions,
  ) => {
    const snapshot = await configuration.getConfiguration(
      {
        paths: executionOpportunityPaths,
        view: ConfigurationView.ACTIVE,
        exposureCeiling: ConfigExposure.DEVELOPER,
      },
      options,
    );
    const values = executionOpportunityPaths.map((path) => {
      const entry = snapshot.entries.find((value) => value.path === path);
      if (!entry?.value)
        throw new Error(`Active Core execution policy unavailable: ${path}`);
      return [
        path.slice("core_execution.opportunity.".length),
        toJson(ValueSchema, entry.value),
      ];
    });
    return executionEnvelope(
      executionOpportunitySchema.parse(Object.fromEntries(values)),
      workMs,
    ).responseTimeoutMs;
  };
  return {
    configuration: {
      list: call(configuration.listConfigDescriptors),
      describe: call(async (path: string, options?: CallOptions) =>
        configuration.getConfigDescriptor({ path }, options),
      ),
      get: call(configuration.getConfiguration),
      setSystem: call((input: ConfigurationOverride, options?: CallOptions) =>
        configuration.setSystemOverride(
          { ...input, value: fromJson(ValueSchema, input.value) },
          options,
        ),
      ),
      clearSystem: call((input: ConfigurationMutation, options?: CallOptions) =>
        configuration.clearSystemOverride(input, options),
      ),
      setSubject: call(
        (
          input: ConfigurationOverride & { subjectId: string },
          options?: CallOptions,
        ) =>
          configuration.setSubjectOverride(
            { ...input, value: fromJson(ValueSchema, input.value) },
            options,
          ),
      ),
      clearSubject: call(
        (
          input: ConfigurationMutation & { subjectId: string },
          options?: CallOptions,
        ) => configuration.clearSubjectOverride(input, options),
      ),
    },
    identity: {
      addresses: call(identity.getIdentityAddresses),
      bind: call(identity.bindIdentity),
      resolve: call(identity.resolveIdentity),
      rebindEntity: call(identity.rebindEntity),
    },
    model: {
      formFromObservation: call(model.formFromObservation, (_input, options) =>
        executionTimeout(undefined, options),
      ),
      deriveMaterial: call(model.deriveMaterial, (_input, options) =>
        executionTimeout(undefined, options),
      ),
      prepareEmbeddings: call(model.prepareEmbeddings, (_input, options) =>
        executionTimeout(undefined, options),
      ),
    },
    resources: {
      materialize: call(resources.materializeResource, (_input, options) =>
        executionTimeout(undefined, options),
      ),
      put: call(registry.putResource),
      get: call(registry.getResource),
      list: call(registry.listResources),
      remove: call(registry.removeResource),
    },
    concepts: {
      createTag: call(concepts.createTag),
      getTag: call(concepts.getTag),
      listTags: call(concepts.listTags),
      searchTags: call(concepts.searchTags),
      reviseTag: call(concepts.reviseTag),
      mergeTags: call(concepts.mergeTags),
      splitTag: call(concepts.splitTag),
      associate: call(concepts.createAssociation),
      revokeAssociation: call(concepts.revokeAssociation),
      neighborhood: call(concepts.getNeighborhood),
      createSchema: call(concepts.createCognitiveSchema),
      getSchema: call(concepts.getCognitiveSchema),
      getSchemaRevision: call(concepts.getCognitiveSchemaRevision),
      addSchemaEvidence: call(concepts.addSchemaEvidence),
      reviseSchema: call(concepts.reviseCognitiveSchema),
      splitSchema: call(concepts.splitCognitiveSchema),
      mergeSchemas: call(concepts.mergeCognitiveSchemas),
      suppressSchema: call(concepts.suppressCognitiveSchema),
      restoreSchema: call(concepts.restoreCognitiveSchema),
      withdrawSchema: call(concepts.withdrawCognitiveSchema),
      reacceptSchema: call(concepts.reacceptCognitiveSchema),
      purgeSchema: call(concepts.purgeCognitiveSchema),
    },
    system: {
      status: call(system.getStatus),
      capabilities: call(system.getCapabilities),
      projections: call(system.getProjectionStatus),
    },
    subjects: {
      create: call(subjects.createSubject),
      get: call(subjects.getSubject),
      list: call(subjects.listSubjects),
      seed: call(subjects.getCognitiveSeed),
      adoptSeed: call(subjects.adoptCognitiveSeed),
    },
    cognition: {
      openSession: call(runtime.openSession),
      getSession: call(runtime.getSession),
      listSessions: call(runtime.listSessions),
      closeSession: call(runtime.closeSession),
      observe: call(runtime.recordObservation),
      query: call(cognition.query, (_input, options) =>
        executionTimeout(undefined, options),
      ),
      prepareQuery: call(cognition.prepareQuery, (_input, options) =>
        executionTimeout(undefined, options),
      ),
      reportUse: call(runtime.reportUse),
      grantMaintenance: call(cognition.grantMaintenance, (input, options) =>
        executionTimeout(input.maxElapsedMs, options),
      ),
      recall: async (
        subjectId: string,
        nousql: string,
        options?: RequestOptions,
      ) => {
        return call(cognition.query, (_input, callOptions) =>
          executionTimeout(undefined, callOptions),
        )({ subjectId, nousql }, options);
      },
      createWorkContext: call(runtime.createWorkContext),
      getWorkContext: call(runtime.getWorkContext),
      listWorkContexts: call(runtime.listWorkContexts),
      updateWorkContext: call(runtime.updateWorkContext),
      pauseWorkContext: call(runtime.pauseWorkContext),
      resumeWorkContext: call(runtime.resumeWorkContext),
      endWorkContext: call(runtime.endWorkContext),
      setActiveWorkContext: call(runtime.setActiveWorkContext),
      project: call(cognition.buildProjection, (_input, options) =>
        executionTimeout(undefined, options),
      ),
      managedContext: call(cognition.buildManagedContext, (_input, options) =>
        executionTimeout(undefined, options),
      ),
    },
    memory: {
      setAccessibility: call(memory.setAccessibility),
      linkRevisions: call(memory.linkRevisions),
      get: call(memory.getMemory),
      list: call(memory.listMemories),
      revision: call(memory.getMemoryRevision),
      history: call(memory.listMemoryRevisions),
      form: call(memory.formMemory),
      revise: call(memory.reviseMemory),
      suppress: call(memory.suppressMemory),
      restore: call(memory.restoreMemory),
      withdraw: call(memory.withdrawMemory),
      reaccept: call(memory.reacceptMemory),
      purge: call(memory.purgeMemory),
      createEpisode: call(memory.createEpisode),
      getEpisode: call(memory.getEpisode),
      getEpisodeRevision: call(memory.getEpisodeRevision),
      listEpisodes: call(memory.listEpisodes),
      listEpisodeRevisions: call(memory.listEpisodeRevisions),
      reviseEpisode: call(memory.reviseEpisode),
      linkEpisodeRevisions: call(memory.linkEpisodeRevisions),
      suppressEpisode: call(memory.suppressEpisode),
      restoreEpisode: call(memory.restoreEpisode),
      withdrawEpisode: call(memory.withdrawEpisode),
      reacceptEpisode: call(memory.reacceptEpisode),
      purgeEpisode: call(memory.purgeEpisode),
      getJournal: call(memory.getJournal),
      getJournalRevision: call(memory.getJournalRevision),
      listJournals: call(memory.listJournals),
      listJournalRevisions: call(memory.listJournalRevisions),
      suppressJournal: call(memory.suppressJournal),
      restoreJournal: call(memory.restoreJournal),
      withdrawJournal: call(memory.withdrawJournal),
      reacceptJournal: call(memory.reacceptJournal),
      purgeJournal: call(memory.purgeJournal),
    },
    material: {
      occurrences: call(material.listOccurrences),
      derivedRegion: call(material.getDerivedRegion),
      producer: call(material.getProducer),
      representations: call(material.listDerivedRepresentations),
      limits: call(material.getLimits),
      getArtifact: call(material.getArtifact),
      listArtifacts: call(material.listArtifacts),
      materialize: call(material.materializeEvidence),
      occurrence: call(material.getOccurrence),
      sourceRegion: call(material.getSourceRegion),
      representation: call(material.getDerivedRepresentation),
    },
  };
}
export type NousClient = ReturnType<typeof createNousClient>;

/** CLI and management callers use one JSON syntax for scalar and structured values. */
export function configurationValue(text: string) {
  return JSON.parse(text) as JsonValue;
}

// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { toJson, type DescService } from "@bufbuild/protobuf";
import { ValueSchema } from "@bufbuild/protobuf/wkt";
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
  type Client,
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
import { SelfService } from "@nous-wave/protocol/nous/wave/v1alpha1/self_pb.js";
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

/** Client namespaces use the canonical service descriptors. */
export type ServiceDescriptors = {
  readonly subjects: typeof SubjectService;
  readonly self: typeof SelfService;
  readonly runtime: typeof RuntimeService;
  readonly cognition: typeof CognitionService;
  readonly memory: typeof MemoryService;
  readonly material: typeof MaterialService;
  readonly identity: typeof IdentityService;
  readonly model: typeof ModelService;
  readonly configuration: typeof ConfigurationService;
  readonly resourceRegistry: typeof ResourceRegistryService;
  readonly resources: typeof ResourceService;
  readonly concepts: typeof ConceptService;
  readonly system: typeof SystemService;
};
export const serviceDescriptors: ServiceDescriptors = {
  subjects: SubjectService,
  self: SelfService,
  runtime: RuntimeService,
  cognition: CognitionService,
  memory: MemoryService,
  material: MaterialService,
  identity: IdentityService,
  model: ModelService,
  configuration: ConfigurationService,
  resourceRegistry: ResourceRegistryService,
  resources: ResourceService,
  concepts: ConceptService,
  system: SystemService,
};
export type NousClient = {
  [K in keyof typeof serviceDescriptors]: Client<
    (typeof serviceDescriptors)[K]
  >;
};

export interface RequestOptions {
  signal?: AbortSignal;
  timeoutMs?: number;
}
export function createNousClient(transport: Transport): NousClient {
  const configuration = owned(ConfigurationService);
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
  function owned<S extends DescService>(
    service: S,
    modelWork = false,
  ): Client<S> {
    const client = createClient(service, transport);
    const methods = client as unknown as Record<
      string,
      (input: object, options?: CallOptions) => Promise<unknown>
    >;
    for (const method of service.methods) {
      const invoke = methods[method.localName]!;
      methods[method.localName] = async (input, options) => {
        try {
          const timeoutMs =
            options?.timeoutMs ??
            (modelWork
              ? await executionTimeout(
                  method.name === "GrantMaintenance"
                    ? (input as { maxElapsedMs?: number }).maxElapsedMs
                    : undefined,
                  options,
                )
              : undefined);
          return await invoke(
            input,
            timeoutMs === undefined ? options : { ...options, timeoutMs },
          );
        } catch (error) {
          if (error instanceof ConnectError) throw new NousError(error);
          throw error;
        }
      };
    }
    return client;
  }
  return Object.fromEntries(
    Object.entries(serviceDescriptors).map(([name, service]) => [
      name,
      service === ConfigurationService
        ? configuration
        : owned(
            service,
            service === CognitionService ||
              service === ModelService ||
              service === ResourceService,
          ),
    ]),
  ) as NousClient;
}

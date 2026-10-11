// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { KernelProjectionService } from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/workflow_pb.js";
import { KernelMaintenanceService } from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/workflow_pb.js";
import { KernelQueryService } from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/workflow_pb.js";
import { SystemService } from "@nous-wave/protocol/nous/wave/v1alpha1/management_pb.js";
import { ResourceRegistryService } from "@nous-wave/protocol/nous/wave/v1alpha1/management_pb.js";
import { RuntimeService } from "@nous-wave/protocol/nous/wave/v1alpha1/services_pb.js";
import { IdentityService } from "@nous-wave/protocol/nous/wave/v1alpha1/identity_pb.js";
import { ConceptService } from "@nous-wave/protocol/nous/wave/v1alpha1/management_pb.js";
import { MemoryService } from "@nous-wave/protocol/nous/wave/v1alpha1/services_pb.js";
import { SubjectService } from "@nous-wave/protocol/nous/wave/v1alpha1/services_pb.js";
import { MaterialService } from "@nous-wave/protocol/nous/wave/v1alpha1/services_pb.js";
import {
  coreExecutionSchema,
  type CoreExecutionPolicy,
} from "./configuration/catalog.js";
import { createClient, type Transport } from "@connectrpc/connect";
import { createGrpcTransport } from "@connectrpc/connect-node";
import { ArtifactStreamService } from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/kernel_pb.js";
import { Health } from "@nous-wave/protocol/grpc/health/v1/health_pb.js";
import {
  KernelModelWorkflowService,
  KernelMaterialWorkflowService,
} from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/model_pb.js";

import { ConfigurationService } from "@nous-wave/protocol/nous/wave/v1alpha1/configuration_pb.js";

import { KernelConfigurationService } from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/configuration_pb.js";
const MAX_WORKFLOW_RPC_BYTES = 8 * 1024 * 1024 + 65536;

export class KernelClient {
  readonly modelWorkflow;
  readonly materialWorkflow;
  readonly hostRuntime;
  readonly projection;
  readonly maintenance;
  readonly queryWorkflow;
  readonly system;
  readonly resourceRegistry;
  readonly runtime;
  readonly configuration;

  readonly identity;
  readonly concepts;
  readonly memory;
  readonly subjects;
  readonly material;
  readonly artifacts;
  readonly health;

  constructor(
    transport: Transport,
    readonly execution: CoreExecutionPolicy = coreExecutionSchema.parse(
      undefined,
    ),
  ) {
    this.runtime = createClient(RuntimeService, transport);
    this.resourceRegistry = createClient(ResourceRegistryService, transport);
    this.system = createClient(SystemService, transport);
    this.queryWorkflow = createClient(KernelQueryService, transport);
    this.maintenance = createClient(KernelMaintenanceService, transport);
    this.projection = createClient(KernelProjectionService, transport);
    this.modelWorkflow = createClient(KernelModelWorkflowService, transport);
    this.materialWorkflow = createClient(
      KernelMaterialWorkflowService,
      transport,
    );
    this.hostRuntime = createClient(KernelConfigurationService, transport);
    this.configuration = createClient(ConfigurationService, transport);
    this.material = createClient(MaterialService, transport);
    this.subjects = createClient(SubjectService, transport);
    this.memory = createClient(MemoryService, transport);
    this.concepts = createClient(ConceptService, transport);
    this.identity = createClient(IdentityService, transport);
    this.artifacts = createClient(ArtifactStreamService, transport);
    this.health = createClient(Health, transport);
  }

  static connect(
    endpoint: string,
    token: string,
    execution = coreExecutionSchema.parse(undefined),
  ) {
    return new KernelClient(
      createGrpcTransport({
        baseUrl: endpoint,
        defaultTimeoutMs: execution.kernel_rpc_timeout_ms,
        readMaxBytes: MAX_WORKFLOW_RPC_BYTES,
        writeMaxBytes: MAX_WORKFLOW_RPC_BYTES,
        interceptors: [
          (next) => async (request) => {
            request.header.set("authorization", "Bearer " + token);
            return next(request);
          },
        ],
      }),
      execution,
    );
  }
}

// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { create } from "@bufbuild/protobuf";
import {
  type GetOperationStatusRequest,
  GetOperationStatusResponseSchema,
  OperationState,
} from "@nous-wave/protocol/nous/wave/v1alpha1/management_pb.js";
import {
  type RebindEntityRequest,
  RebindEntityRequestSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/identity_pb.js";
import { RepresentationListResponseSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/management_pb.js";
import {
  type AdoptCognitiveSeedRequest,
  CognitiveSeedVersionSchema,
  type ListRequest,
  ListArtifactsResponseSchema,
  MaterialLimitsSchema,
  type ObjectRequest,
  type RepresentationListRequest,
  ProducerSignatureSchema,
  type SubjectRequest,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import type { connectNousInstance } from "@nous-wave/client/node";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { expect, it, vi } from "vitest";
import { workspacePaths } from "../../../scripts/workspace.js";
import { runCli } from "./support/client.js";

it("exposes typed Subject read and seed adoption commands", async () => {
  const root = await mkdtemp(join(workspacePaths.temporary, "subject-leaves-"));
  const subjectId = "33333333-3333-4333-8333-333333333333";
  const get = vi.fn(async (_request: SubjectRequest) => ({
    subjectId,
    status: "active",
  }));
  const seed = vi.fn(async (_request: SubjectRequest) =>
    create(CognitiveSeedVersionSchema, { subjectId, format: "text/plain" }),
  );
  const adoptSeed = vi.fn(async (_request: AdoptCognitiveSeedRequest) =>
    create(CognitiveSeedVersionSchema, {
      subjectId,
      adoptionKind: "import",
      format: "application/vnd.nous-wave.cognitive-seed+toml;version=1",
    }),
  );
  const requestFile = join(root, "seed.toml");
  await writeFile(
    requestFile,
    [
      'text = "schema_version = 1\\n"',
      'format = "application/vnd.nous-wave.cognitive-seed+toml;version=1"',
      'kind = "import"',
      "",
    ].join("\n"),
  );
  const connect = async () =>
    ({ subjects: { get, seed, adoptSeed } }) as unknown as Awaited<
      ReturnType<typeof connectNousInstance>
    >;
  try {
    await runCli(
      [
        "subject",
        "get",
        subjectId,
        "--run-root",
        root,
        "--instance-root",
        root,
      ],
      connect,
    );
    await runCli(
      [
        "subject",
        "seed",
        subjectId,
        "--run-root",
        root,
        "--instance-root",
        root,
      ],
      connect,
    );
    await runCli(
      [
        "subject",
        "adopt-seed",
        subjectId,
        "--request-file",
        requestFile,
        "--run-root",
        root,
        "--instance-root",
        root,
      ],
      connect,
    );
    expect(get.mock.calls[0]![0].subjectId).toBe(subjectId);
    expect(seed.mock.calls[0]![0].subjectId).toBe(subjectId);
    const adoption = adoptSeed.mock.calls[0]![0];
    expect(adoption.subjectId).toBe(subjectId);
    expect(adoption.operationId).toMatch(/^[0-9a-f-]{36}$/i);
    expect(adoption.kind).toBe("import");
    expect(adoption.seed?.text).toBe("schema_version = 1\n");
    expect(adoption.seed?.format).toBe(
      "application/vnd.nous-wave.cognitive-seed+toml;version=1",
    );
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

it("constructs a typed server operation identity for direct inspection", async () => {
  const root = await mkdtemp(
    join(workspacePaths.temporary, "status-identity-"),
  );
  const subjectId = "44444444-4444-4444-8444-444444444444";
  const operationId = crypto.randomUUID();
  const operationStatus = vi.fn(async (_request: GetOperationStatusRequest) =>
    create(GetOperationStatusResponseSchema, {
      state: OperationState.IN_PROGRESS,
    }),
  );
  const connect = async () =>
    ({ system: { operationStatus } }) as unknown as Awaited<
      ReturnType<typeof connectNousInstance>
    >;
  try {
    const result = await runCli(
      [
        "operation",
        "inspect",
        "mutation",
        "revise_memory",
        subjectId,
        operationId,
        "--run-root",
        root,
        "--instance-root",
        root,
      ],
      connect,
    );
    const inspectedRequest = operationStatus.mock.calls[0]![0];
    expect(inspectedRequest.identity.case).toBe("mutationReceipt");
    if (inspectedRequest.identity.case === "mutationReceipt") {
      expect(inspectedRequest.identity.value.subjectId).toBe(subjectId);
      expect(inspectedRequest.identity.value.operationId).toBe(operationId);
      expect(inspectedRequest.identity.value.operationKind).toBe(
        "revise_memory",
      );
    }
    expect(result).toMatchObject({
      kind: "operation.inspect",
      data: {
        identity: {
          case: "mutationReceipt",
          subjectId,
          operationId,
          operationKind: "revise_memory",
        },
        serverState: "IN_PROGRESS",
        certainty: "in_progress",
      },
    });
    expect(result).not.toHaveProperty("data.request");
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

it("uses generated material limits request and typed entity rebind request", async () => {
  const root = await mkdtemp(
    join(workspacePaths.temporary, "material-leaves-"),
  );
  const subjectId = "55555555-5555-4555-8555-555555555555";
  const mentionId = "66666666-6666-4666-8666-666666666666";
  const limits = vi
    .fn()
    .mockResolvedValue(create(MaterialLimitsSchema, { maxUploadBytes: 1024n }));
  const rebindEntity = vi.fn(async (_request: RebindEntityRequest) => {});
  const connect = async () =>
    ({
      material: { limits },
      identity: { rebindEntity },
    }) as unknown as Awaited<ReturnType<typeof connectNousInstance>>;
  try {
    await runCli(
      ["material", "limits", "--run-root", root, "--instance-root", root],
      connect,
    );
    await runCli(
      [
        "identity",
        "rebind",
        mentionId,
        "--subject",
        subjectId,
        "--binding-state",
        "unbound",
        "--reason",
        "reviewed",
        "--run-root",
        root,
        "--instance-root",
        root,
      ],
      connect,
    );
    expect(limits).toHaveBeenCalledOnce();
    expect(rebindEntity).toHaveBeenCalledWith(
      create(RebindEntityRequestSchema, {
        subjectId,
        mentionId,
        bindingState: "unbound",
        reason: "reviewed",
      }),
    );
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

it("exposes bounded artifact and derived-representation reads", async () => {
  const root = await mkdtemp(join(workspacePaths.temporary, "material-reads-"));
  const subjectId = "77777777-7777-4777-8777-777777777777";
  const artifactId = "88888888-8888-4888-8888-888888888888";
  const producerId = "99999999-9999-4999-8999-999999999999";
  const listArtifacts = vi.fn(async (_request: ListRequest) =>
    create(ListArtifactsResponseSchema),
  );
  const representations = vi.fn(async (_request: RepresentationListRequest) =>
    create(RepresentationListResponseSchema),
  );
  const producer = vi.fn(async (_request: ObjectRequest) =>
    create(ProducerSignatureSchema),
  );
  const connect = async () =>
    ({
      material: { listArtifacts, representations, producer },
    }) as unknown as Awaited<ReturnType<typeof connectNousInstance>>;
  try {
    await runCli(
      [
        "material",
        "artifacts",
        "--subject",
        subjectId,
        "--page-size",
        "12",
        "--page-token",
        "next",
        "--run-root",
        root,
        "--instance-root",
        root,
      ],
      connect,
    );
    await runCli(
      [
        "material",
        "representations",
        "--subject",
        subjectId,
        "--artifact",
        artifactId,
        "--kind",
        "summary",
        "--limit",
        "8",
        "--run-root",
        root,
        "--instance-root",
        root,
      ],
      connect,
    );
    await runCli(
      [
        "material",
        "producer",
        producerId,
        "--subject",
        subjectId,
        "--run-root",
        root,
        "--instance-root",
        root,
      ],
      connect,
    );
    const artifactsRequest = listArtifacts.mock.calls[0]![0];
    expect(artifactsRequest.subjectId).toBe(subjectId);
    expect(artifactsRequest.page?.pageSize).toBe(12);
    expect(artifactsRequest.page?.pageToken).toBe("next");
    const representationsRequest = representations.mock.calls[0]![0];
    expect(representationsRequest.subjectId).toBe(subjectId);
    expect(representationsRequest.artifactId).toBe(artifactId);
    expect(representationsRequest.kind).toBe("summary");
    expect(representationsRequest.limit).toBe(8);
    expect(producer.mock.calls[0]![0].subjectId).toBe(subjectId);
    expect(producer.mock.calls[0]![0].id).toBe(producerId);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

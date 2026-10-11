// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { execFile } from "node:child_process";
import { promisify } from "node:util";
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { rm, readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";
import {
  ConfigExposure,
  ConfigurationView,
  configurationValue,
} from "@nous-wave/client";
import { create, fromJson, toJson, type JsonValue } from "@bufbuild/protobuf";
import { ValueSchema } from "@bufbuild/protobuf/wkt";
import { CreateSubjectRequestSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { ListConfigDescriptorsResponseSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/configuration_pb.js";
import { boot, prepare, stop, type Boot } from "./support.js";
import { workspaceTemp } from "../workspace.js";
import { resolveLocations } from "../../apps/nous-core/src/locations.js";

const root = await workspaceTemp("smoke", "configuration-");
let current: Boot | undefined;
try {
  const locator = await prepare(root);
  const locations = await resolveLocations({
    locator,
    installationHome: process.cwd(),
  });
  const configPath = join(locations.config, "nous.toml");
  await writeFile(
    configPath,
    (await readFile(configPath, "utf8")) +
      "\n[maintenance]\nenabled = false\n[video]\nmax_frames = 6\n",
  );
  current = await boot(locator);
  let client = current.client;
  const subjectRequestPath = join(root, "subject.toml");
  await writeFile(
    subjectRequestPath,
    [
      "[cognitive_seed]",
      'text = "schema_version = 1"',
      'format = "application/vnd.nous-wave.cognitive-seed+toml;version=1"',
      "",
    ].join("\n"),
  );
  const firstSubject = await promisify(execFile)(
    process.execPath,
    [
      "--import",
      import.meta.resolve("tsx"),
      join(locations.program, "apps/nous-core/src/launcher.ts"),
      "--development",
      "--locator",
      locator,
      "subject",
      "create",
      "--request-file",
      subjectRequestPath,
    ],
    { maxBuffer: 2 * 1024 * 1024, windowsHide: true },
  );
  assert.match(
    firstSubject.stdout,
    /subjectId: sub:[bdfghjklmnprstvz][aiou][bdfghjklmnprstvz][aiou][bdfghjklmnprstvz]-/,
  );
  const cli = async (...args: string[]) => {
    const result = await promisify(execFile)(
      process.execPath,
      [
        "--import",
        import.meta.resolve("tsx"),
        join(locations.program, "apps/nous-core/src/launcher.ts"),
        "--development",
        "--locator",
        locator,
        "config",
        ...args,
      ],
      { maxBuffer: 2 * 1024 * 1024, windowsHide: true },
    );
    const parsed = JSON.parse(result.stdout) as {
      schemaVersion?: string;
      data?: unknown;
    };
    if (args[0] !== "check") assert.equal(parsed.schemaVersion, "nous.cli.v2");
    return (args[0] === "check" ? parsed : parsed.data) as {
      valid?: boolean;
      change?: unknown;
      descriptors?: { path: string; exposure: string }[];
    };
  };
  const cliDescriptors = async (...args: string[]) =>
    fromJson(
      ListConfigDescriptorsResponseSchema,
      (await cli(...args)) as JsonValue,
    ).descriptors;
  assert.equal((await cli("check")).valid, true);
  const standardCli = await cliDescriptors("list", "--json");
  assert(standardCli.length > 0);
  assert(standardCli.every((d) => d.exposure === ConfigExposure.STANDARD));
  const advancedCli = await cliDescriptors("list", "--advanced", "--json");
  assert(advancedCli.some((d) => d.exposure === ConfigExposure.ADVANCED));
  assert(advancedCli.every((d) => d.exposure !== ConfigExposure.DEVELOPER));
  const developerCli = await cliDescriptors("list", "--developer", "--json");
  assert(developerCli.some((d) => d.exposure === ConfigExposure.DEVELOPER));
  const standard = await client.configuration.list({});
  assert(standard.descriptors.some((d) => d.path === "models"));
  assert(standard.descriptors.some((d) => d.path === "maintenance.enabled"));
  assert(
    standard.descriptors.every((d) => d.exposure === ConfigExposure.STANDARD),
  );
  const schemaValue = (
    await client.configuration.describe("runtime.resident_limit")
  ).jsonSchema;
  const schema = schemaValue && (toJson(ValueSchema, schemaValue) as JsonValue);
  assert(schema && typeof schema === "object" && !Array.isArray(schema));
  assert.equal(
    (schema as Record<string, unknown>).$schema,
    "https://json-schema.org/draft/2020-12/schema",
  );
  const subjectId = randomUUID();
  const subject = await client.subjects.create(
    create(CreateSubjectRequestSchema, {
      subjectId,
      operationId: randomUUID(),
      cognitiveSeed: {
        text: "schema_version = 1",
        format: "application/vnd.nous-wave.cognitive-seed+toml;version=1",
      },
    }),
  );
  const subjectAddress = await client.identity.bind({
    subjectId: subject.subjectId,
    canonical: { kind: "subject", value: subject.subjectId },
    displayName: "Configuration smoke",
    aliases: ["Original label"],
  });
  const addresses = await client.identity.addresses({
    targets: [
      {
        subjectId: subject.subjectId,
        canonical: { kind: "subject", value: subject.subjectId },
      },
      {
        subjectId: subject.subjectId,
        canonical: { kind: "entity", value: "entity:unallocated" },
      },
      {
        subjectId: randomUUID(),
        canonical: { kind: "subject", value: subject.subjectId },
      },
    ],
  });
  assert.deepEqual(
    addresses.addresses.map((address) => ({
      status: address.status,
      lexicalRef: address.lexicalRef,
    })),
    [
      { status: "BOUND", lexicalRef: subjectAddress.lexicalRef },
      { status: "UNKNOWN_REFERENCE", lexicalRef: undefined },
      { status: "UNKNOWN_REFERENCE", lexicalRef: undefined },
    ],
  );
  const labels = await client.identity.resolve({
    subjectId: subject.subjectId,
    kind: "subject",
    locator: { case: "lexicalRef", value: subjectAddress.lexicalRef },
  });
  assert.equal(labels.candidates[0]?.displayName, "Configuration smoke");
  assert.deepEqual(labels.candidates[0]?.aliases, ["Original label"]);
  const entry = async (
    path: string,
    targetSubject?: string,
    desired = false,
  ) => {
    const result = await client.configuration.get({
      paths: [path],
      subjectId: targetSubject,
      view: desired ? ConfigurationView.DESIRED : ConfigurationView.ACTIVE,
    });
    const item = result.entries[0];
    assert(item);
    return {
      ...item,
      value: item.value ? toJson(ValueSchema, item.value) : undefined,
    };
  };
  const revision = async () =>
    (
      await client.configuration.get({
        view: ConfigurationView.DESIRED,
        paths: ["maintenance.enabled"],
      })
    ).configurationRevision;
  const defaults = (await entry("video.max_frames")).value;
  const modelGraph = {
    gateway_profiles: {
      local: {
        base_url: "http://127.0.0.1:1/v1",
        credential_env: "NOUS_CONFIGURATION_SMOKE",
      },
    },
    model_profiles: {
      chat: {
        gateway: "local",
        protocol: "openai-chat",
        model: "declared",
        capabilities: ["text", "structured_output"],
      },
    },
    execution_profiles: {
      primary: {
        model: "chat",
        provider_options: {
          test: {
            $typeName: "original JSON",
            $unknown: ["memory:33333333-3333-4333-8333-333333333333"],
            subjectId: "33333333-3333-4333-8333-333333333333",
          },
        },
      },
    },
    roles: { memory_formation: { routes: ["primary"] } },
  };
  const graphOperation = randomUUID();
  const graphRevision = (
    await client.configuration.get({
      view: ConfigurationView.DESIRED,
      paths: ["models"],
    })
  ).configurationRevision;
  const sparseGraph = await client.configuration.setSystem({
    operationId: graphOperation,
    path: "models",
    value: configurationValue(JSON.stringify(modelGraph)),
    expectedRevision: graphRevision,
  });
  const frozenGraph = (await entry("models", undefined, true)).value;
  assert(
    frozenGraph &&
      typeof frozenGraph === "object" &&
      !Array.isArray(frozenGraph),
  );
  assert.deepEqual(
    (frozenGraph.execution_profiles as Record<string, unknown>).primary,
    {
      model: "chat",
      reasoning: "provider-default",
      provider_options: modelGraph.execution_profiles.primary.provider_options,
      max_output_tokens: 4096,
      timeout_ms: 30000,
    },
  );
  const explicitGraph = await client.configuration.setSystem({
    operationId: graphOperation,
    path: "models",
    value: configurationValue(JSON.stringify(frozenGraph)),
    expectedRevision: graphRevision,
  });
  assert.equal(explicitGraph.revision, sparseGraph.revision);
  assert.equal(explicitGraph.desiredDigest, sparseGraph.desiredDigest);
  await assert.rejects(
    client.configuration.setSystem({
      operationId: randomUUID(),
      path: "models",
      value: configurationValue(JSON.stringify(modelGraph)),
      expectedRevision: graphRevision,
    }),
    /revision.*changed|revision.*conflict/i,
  );
  assert.deepEqual((await entry("models", undefined, true)).value, frozenGraph);
  await assert.rejects(
    client.configuration.setSystem({
      operationId: randomUUID(),
      expectedRevision: await revision(),
      path: "models",
      value: configurationValue(
        JSON.stringify({
          ...modelGraph,
          roles: { memory_formation: { routes: ["absent"] } },
        }),
      ),
    }),
    /models.roles.memory_formation.routes.0/,
  );
  assert.equal(defaults, 6);
  const normalizedOperation = randomUUID();
  const videoRevision = await revision();
  const omitted = await client.configuration.setSystem({
    operationId: normalizedOperation,
    expectedRevision: videoRevision,
    path: "video.max_frames",
    value: configurationValue("4"),
  });
  const explicit = await client.configuration.setSystem({
    operationId: normalizedOperation,
    expectedRevision: videoRevision,
    path: "video.max_frames",
    value: configurationValue("4"),
  });
  assert.equal(omitted.desiredDigest, explicit.desiredDigest);
  assert.equal(omitted.revision, explicit.revision);
  assert.equal((await entry("video.max_frames", undefined, true)).value, 4);
  assert.equal((await entry("video.max_frames")).value, defaults);
  assert.equal(
    (await entry("video.input_mode", undefined, true)).value,
    "direct",
  );
  assert.equal((await entry("video.ffmpeg_executable")).value, null);
  assert.equal(
    (await client.configuration.describe("video.max_frames")).semanticEffect,
    "authority_formation",
  );
  await assert.rejects(client.configuration.describe("video"));
  await assert.rejects(
    client.configuration.setSystem({
      operationId: randomUUID(),
      expectedRevision: await revision(),
      path: "video.max_frames",
      value: configurationValue("17"),
    }),
    /maximum|16/,
  );
  assert.equal((await entry("maintenance.enabled")).source, "deployment_file");
  const operationId = randomUUID();
  const maintenanceRevision = await revision();
  const change = await client.configuration.setSystem({
    operationId,
    expectedRevision: maintenanceRevision,
    path: "maintenance.enabled",
    value: configurationValue("true"),
  });
  const replay = await client.configuration.setSystem({
    operationId,
    expectedRevision: maintenanceRevision,
    path: "maintenance.enabled",
    value: configurationValue("true"),
  });
  assert.equal(replay.revision, change.revision);
  assert.equal((await entry("maintenance.enabled")).value, true);
  await assert.rejects(
    cli(
      "set",
      "maintenance.enabled",
      "false",
      "--expected-revision",
      String(await revision()),
      "--json",
    ),
    /value-json/,
  );
  await assert.rejects(
    cli(
      "clear",
      "maintenance.enabled",
      "--expected-revision",
      maintenanceRevision.toString(),
      "--json",
    ),
    /revision.*changed/i,
  );
  const cliOperation = randomUUID();
  const cliRevision = await revision();
  const cliArgs = [
    "set",
    "maintenance.enabled",
    "--value-json",
    "true",
    "--operation-id",
    cliOperation,
    "--expected-revision",
    cliRevision.toString(),
    "--json",
  ];
  const cliChange = await cli(...cliArgs);
  assert(cliChange.change);
  await client.configuration.setSubject({
    operationId: randomUUID(),
    expectedRevision: await revision(),
    subjectId: subject.subjectId,
    path: "maintenance.enabled",
    value: configurationValue("false"),
  });
  assert.equal(
    (await entry("maintenance.enabled", subject.subjectId)).source,
    "persisted_subject",
  );
  assert.equal(
    (await entry("maintenance.enabled", subject.subjectId)).value,
    false,
  );
  const beforeReplay = await revision();
  assert.deepEqual((await cli(...cliArgs)).change, cliChange.change);
  assert.equal(await revision(), beforeReplay);
  const initial = (await entry("runtime.resident_limit")).value;
  const restart = await client.configuration.setSystem({
    operationId: randomUUID(),
    expectedRevision: await revision(),
    path: "runtime.resident_limit",
    value: configurationValue("512"),
  });
  assert.deepEqual(restart.effects, ["restart_required"]);
  assert.equal((await entry("runtime.resident_limit")).value, initial);
  assert.equal(
    (await entry("runtime.resident_limit", undefined, true)).value,
    512,
  );
  assert.equal(
    (await entry("runtime.resident_limit")).pendingEffect,
    "restart_required",
  );
  await assert.rejects(
    client.configuration.setSystem({
      operationId: randomUUID(),
      expectedRevision: await revision(),
      path: "host.port",
      value: configurationValue("0"),
    }),
  );
  await stop(current);
  current = await boot(locator);
  client = current.client;
  assert.equal((await entry("runtime.resident_limit")).value, 512);
  assert.equal((await entry("video.max_frames")).value, 4);
  assert.equal((await entry("video.max_frames")).source, "persisted_system");
  await client.configuration.clearSystem({
    operationId: randomUUID(),
    path: "video.max_frames",
    expectedRevision: await revision(),
  });
  assert.equal((await entry("video.max_frames", undefined, true)).value, 6);
  assert.equal(
    (await entry("video.max_frames", undefined, true)).source,
    "deployment_file",
  );
  assert.equal((await entry("video.max_frames")).value, 4);
  assert.equal(
    (await entry("runtime.resident_limit")).pendingEffect,
    undefined,
  );
  assert.equal(
    (await entry("maintenance.enabled", subject.subjectId)).value,
    false,
  );
  console.log(
    "CONFIGURATION_SMOKE catalog=true cli=true precedence=true replay=true restart=true normalized_identity=true revision_cas=true readonly_address_batch=true",
  );
} finally {
  await stop(current);
  await rm(root, { recursive: true, force: true });
}

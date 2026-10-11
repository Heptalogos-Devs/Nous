// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { create } from "@bufbuild/protobuf";
import {
  ConfigExposure,
  ConfigurationView,
  configurationValue,
} from "@nous-wave/client";
import { GetConfigurationRequestSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/configuration_pb.js";
import type { CliEnvironment } from "../runtime.js";
import { CliError } from "../agent.js";

export async function configCommands(
  env: CliEnvironment,
  action: string | undefined,
  argument?: string,
) {
  const { client, values, required } = env;
  const exposureCeiling = values.developer
    ? ConfigExposure.DEVELOPER
    : values.advanced
      ? ConfigExposure.ADVANCED
      : ConfigExposure.STANDARD;
  if (action === "list") return client.configuration.list({ exposureCeiling });
  const path = required(argument, "Configuration path");
  if (action === "describe") return client.configuration.describe(path);
  const descriptor = await client.configuration.describe(path);
  const get = (view: ConfigurationView) =>
    client.configuration.get(
      create(GetConfigurationRequestSchema, {
        paths: [path],
        subjectId: values.subject,
        view,
        exposureCeiling,
      }),
    );
  const view = values.view ?? "both";
  if (action === "get") {
    if (!("active" === view || "desired" === view || "both" === view))
      throw new CliError(
        "INVALID_ARGUMENT",
        "--view must be active, desired or both",
      );
    const [active, desired] = await Promise.all([
      view === "desired" ? undefined : get(ConfigurationView.ACTIVE),
      view === "active" ? undefined : get(ConfigurationView.DESIRED),
    ]);
    return {
      path,
      view,
      descriptor: {
        owner: descriptor.owner,
        scopePolicy: descriptor.scopePolicy,
        storagePolicy: descriptor.storagePolicy,
        applyMode: descriptor.applyMode,
        semanticEffect: descriptor.semanticEffect,
        sensitivity: descriptor.sensitivity,
      },
      active,
      desired,
      activeDigest: active?.effectiveDigest,
      desiredDigest: desired?.effectiveDigest,
      revision: desired?.configurationRevision ?? active?.configurationRevision,
    };
  }
  if (action !== "set" && action !== "clear")
    throw new CliError(
      "INVALID_ARGUMENT",
      "Use config list|describe|get|set|clear",
    );
  const operationId = values["operation-id"] ?? crypto.randomUUID();
  const suppliedRevision = required(
    values["expected-revision"],
    "--expected-revision from config get --view desired",
  );
  if (!/^(0|[1-9][0-9]*)$/.test(suppliedRevision))
    throw new CliError(
      "INVALID_ARGUMENT",
      "Expected revision must be a nonnegative integer",
    );
  const expectedRevision = BigInt(suppliedRevision);
  if (expectedRevision > 9223372036854775807n)
    throw new CliError("INVALID_ARGUMENT", "Expected revision exceeds int64");
  let change;
  if (action === "set") {
    const value = configurationValue(
      required(values["value-json"], "--value-json with a complete JSON value"),
    );
    change = values.subject
      ? await client.configuration.setSubject({
          operationId,
          path,
          expectedRevision,
          value,
          subjectId: values.subject,
        })
      : await client.configuration.setSystem({
          operationId,
          path,
          value,
          expectedRevision,
        });
  } else {
    change = values.subject
      ? await client.configuration.clearSubject({
          operationId,
          path,
          expectedRevision,
          subjectId: values.subject,
        })
      : await client.configuration.clearSystem({
          operationId,
          path,
          expectedRevision,
        });
  }
  const [active, desired] = await Promise.all([
    get(ConfigurationView.ACTIVE),
    get(ConfigurationView.DESIRED),
  ]);
  const projections =
    descriptor.applyMode === "serving_rebuild"
      ? await client.system.projections({ subjectId: values.subject ?? "" })
      : undefined;
  return {
    change,
    descriptor: {
      path: descriptor.path,
      owner: descriptor.owner,
      scopePolicy: descriptor.scopePolicy,
      storagePolicy: descriptor.storagePolicy,
      applyMode: descriptor.applyMode,
      semanticEffect: descriptor.semanticEffect,
      sensitivity: descriptor.sensitivity,
    },
    active,
    desired,
    activeDigest: active.effectiveDigest,
    desiredDigest: desired.effectiveDigest,
    projections,
  };
}

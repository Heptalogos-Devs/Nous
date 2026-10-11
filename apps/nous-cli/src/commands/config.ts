// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { defineCommand } from "citty";
import { booleanArgs, commonArgs, stringArgs } from "./command-args.js";
import type { CliLeaf } from "./command-types.js";

import {
  ConfigExposure,
  ConfigurationView,
  configurationValue,
} from "@nous-wave/client";
import type { CliEnvironment } from "../runtime.js";
import { CliError, nonnegativeInt64 } from "../agent.js";

const exposureCeiling = (env: CliEnvironment) =>
  env.values.developer
    ? ConfigExposure.DEVELOPER
    : env.values.advanced
      ? ConfigExposure.ADVANCED
      : ConfigExposure.STANDARD;

async function configurationViews(
  env: CliEnvironment,
  path: string,
  view: string,
) {
  if (!(view === "active" || view === "desired" || view === "both"))
    throw new CliError(
      "INVALID_ARGUMENT",
      "--view must be active, desired or both",
    );
  const get = (projection: ConfigurationView) =>
    env.client.configuration.get({
      paths: [path],
      subjectId: env.values.subject,
      view: projection,
      exposureCeiling: exposureCeiling(env),
    });
  const [active, desired] = await Promise.all([
    view === "desired" ? undefined : get(ConfigurationView.ACTIVE),
    view === "active" ? undefined : get(ConfigurationView.DESIRED),
  ]);
  return { active, desired };
}

async function changeConfiguration(
  env: CliEnvironment,
  argument: string | undefined,
  setting: boolean,
) {
  const { client, values, required } = env;
  const path = required(argument, "Configuration path");
  const descriptor = await client.configuration.describe(path);
  const operationId = values["operation-id"] ?? crypto.randomUUID();
  const expectedRevision = nonnegativeInt64(
    values["expected-revision"],
    "--expected-revision from config get --view desired",
  );
  let change;
  if (setting) {
    const value = configurationValue(
      required(values["value-json"], "--value-json with a complete JSON value"),
    );
    change = values.subject
      ? await client.configuration.setSubject({
          operationId,
          subjectId: values.subject,
          path,
          value,
          expectedRevision,
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
  const { active, desired } = await configurationViews(env, path, "both");
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
    activeDigest: active?.effectiveDigest,
    desiredDigest: desired?.effectiveDigest,
    projections,
  };
}

const configList = (env: CliEnvironment) =>
  env.client.configuration.list({ exposureCeiling: exposureCeiling(env) });
const configDescribe = (env: CliEnvironment, argument?: string) =>
  env.client.configuration.describe(
    env.required(argument, "Configuration path"),
  );
const configGet = async (env: CliEnvironment, argument?: string) => {
  const path = env.required(argument, "Configuration path");
  const descriptor = await env.client.configuration.describe(path);
  const view = env.values.view ?? "both";
  const { active, desired } = await configurationViews(env, path, view);
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
    revision: (
      desired?.configurationRevision ?? active?.configurationRevision
    )?.toString(),
  };
};
const configSet = (env: CliEnvironment, argument?: string) =>
  changeConfiguration(env, argument, true);
const configClear = (env: CliEnvironment, argument?: string) =>
  changeConfiguration(env, argument, false);

export function createConfigCommand(leaf: CliLeaf) {
  return defineCommand({
    args: commonArgs,
    subCommands: {
      list: leaf({ ...booleanArgs("advanced") }, configList, {
        description: "List descriptors at the selected exposure level.",
      }),
      describe: leaf({}, configDescribe, {
        description: "Show one descriptor, schema, scope and effect.",
        parameters: ["<path>"],
      }),
      get: leaf({ ...stringArgs("view") }, configGet, {
        description: "Read active and/or desired configuration projections.",
        parameters: ["<path>", "--view active|desired|both (default both)"],
      }),
      set: leaf(
        { ...stringArgs("value-json", "expected-revision", "operation-id") },
        configSet,
        {
          description:
            "Set a complete JSON value and report active/desired state.",
          parameters: [
            "<path>",
            "--value-json <complete JSON value>",
            "--expected-revision <desired revision>",
          ],
        },
      ),
      clear: leaf(
        { ...stringArgs("expected-revision", "operation-id") },
        configClear,
        {
          description: "Clear an override and report active/desired state.",
          parameters: ["<path>", "--expected-revision <desired revision>"],
        },
      ),
    },
  });
}

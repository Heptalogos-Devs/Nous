// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import {
  ConfigExposure,
  ConfigurationView,
  configurationValue,
} from "@nous-wave/client";
import type { CliEnvironment } from "../runtime.js";

export async function configCommands(
  env: CliEnvironment,
  action: string | undefined,
  argument?: string,
  positionals: string[] = [],
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
  if (action === "get")
    return client.configuration.get({
      paths: [path],
      subjectId: values.subject,
      view: values.desired
        ? ConfigurationView.DESIRED
        : ConfigurationView.ACTIVE,
    });
  if (action !== "set" && action !== "clear")
    throw new Error("Use config list|describe|get|set|clear");
  const operationId = values["operation-id"] ?? crypto.randomUUID();
  const suppliedRevision = required(
    values["expected-revision"],
    "--expected-revision from config get --desired",
  );
  if (!/^(0|[1-9][0-9]*)$/.test(suppliedRevision))
    throw new Error("Expected revision must be a nonnegative integer");
  const expectedRevision = BigInt(suppliedRevision);
  if (expectedRevision > 9223372036854775807n)
    throw new Error("Expected revision exceeds int64");
  if (action === "set") {
    const value = configurationValue(required(positionals[3], "JSON value"));
    return values.subject
      ? client.configuration.setSubject({
          operationId,
          path,
          expectedRevision,
          value,
          subjectId: values.subject,
        })
      : client.configuration.setSystem({
          operationId,
          path,
          value,
          expectedRevision,
        });
  }
  if (action === "clear")
    return values.subject
      ? client.configuration.clearSubject({
          operationId,
          path,
          expectedRevision,
          subjectId: values.subject,
        })
      : client.configuration.clearSystem({
          operationId,
          path,
          expectedRevision,
        });
  throw new Error("Use config list|describe|get|set|clear");
}

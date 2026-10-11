// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { fromJson, toJson, type JsonValue } from "@bufbuild/protobuf";
import { ValueSchema } from "@bufbuild/protobuf/wkt";
import { Code, ConnectError, type ServiceImpl } from "@connectrpc/connect";
import { ConfigurationService } from "@nous-wave/protocol/nous/wave/v1alpha1/configuration_pb.js";
import { normalizeCoreConfigurationValue } from "./catalog.js";
import { z } from "zod";

/** Kernel owns precedence and snapshots; Core only runs the owning TypeScript normalizer. */
export function configurationOperations(
  operations: ServiceImpl<typeof ConfigurationService>,
): ServiceImpl<typeof ConfigurationService> {
  const normalize = (
    path: string,
    value: Parameters<typeof toJson<typeof ValueSchema>>[1] | undefined,
  ) => {
    try {
      return fromJson(
        ValueSchema,
        normalizeCoreConfigurationValue(
          path,
          value ? toJson(ValueSchema, value) : null,
        ) as JsonValue,
      );
    } catch (error) {
      throw new ConnectError(
        error instanceof z.ZodError
          ? error.issues
              .map(
                (issue) =>
                  `${[path, ...issue.path].join(".")}: ${issue.message}`,
              )
              .join("; ")
          : `Invalid configuration ${path}`,
        Code.InvalidArgument,
      );
    }
  };
  return {
    ...operations,
    setSystemOverride: (input, context) =>
      operations.setSystemOverride(
        { ...input, value: normalize(input.path, input.value) },
        context,
      ),
    setSubjectOverride: (input, context) =>
      operations.setSubjectOverride(
        { ...input, value: normalize(input.path, input.value) },
        context,
      ),
  };
}

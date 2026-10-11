// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { create, fromJson } from "@bufbuild/protobuf";
import { Code, ConnectError, type ServiceImpl } from "@connectrpc/connect";
import { ValueSchema } from "@bufbuild/protobuf/wkt";
import { expect, it, vi } from "vitest";
import {
  ConfigurationService,
  SetSystemOverrideRequestSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/configuration_pb.js";
import {
  ErrorCode,
  ErrorDetailSchema,
  ErrorOwner,
  ErrorRecovery,
} from "@nous-wave/protocol/nous/wave/v1alpha1/errors_pb.js";
import { configurationOperations } from "../../src/configuration/service.js";

it("returns a safe typed configuration error with the exact path and trace ID", async () => {
  const log = vi.spyOn(console, "error").mockImplementation(() => {});
  try {
    const service = configurationOperations({
      setSystemOverride: vi.fn(),
    } as unknown as ServiceImpl<typeof ConfigurationService>);
    let caught: unknown;
    try {
      await service.setSystemOverride(
        create(SetSystemOverrideRequestSchema, {
          path: "host.port",
          value: create(ValueSchema, {
            kind: { case: "numberValue", value: 70000 },
          }),
        }),
        {} as never,
      );
    } catch (error) {
      caught = error;
    }
    expect(caught).toBeInstanceOf(ConnectError);
    if (!(caught instanceof ConnectError))
      throw new Error("expected ConnectError");
    expect(caught.code).toBe(Code.InvalidArgument);
    expect(caught.rawMessage).toBe(
      "Invalid configuration value at host.port (maximum 65535)",
    );
    expect(caught.rawMessage).not.toContain("70000");
    const [detail] = caught.findDetails(ErrorDetailSchema);
    expect(detail).toMatchObject({
      code: ErrorCode.INVALID_REQUEST,
      recovery: ErrorRecovery.CORRECT_REQUEST,
      owner: ErrorOwner.CONFIGURATION,
      context: {
        path: "host.port",
        field_path: "host.port",
        validation_code: "too_big",
      },
    });
    expect(detail?.traceId).toMatch(/^[0-9a-f-]{36}$/);
    expect(log).toHaveBeenCalledOnce();
  } finally {
    log.mockRestore();
  }
});

it("reports a cross-field route error at its safe configuration path", async () => {
  const service = configurationOperations({
    setSystemOverride: vi.fn(),
  } as unknown as ServiceImpl<typeof ConfigurationService>);
  let caught: unknown;
  try {
    await service.setSystemOverride(
      create(SetSystemOverrideRequestSchema, {
        path: "models",
        value: fromJson(ValueSchema, {
          roles: { memory_formation: { routes: ["absent"] } },
        }),
      }),
      {} as never,
    );
  } catch (error) {
    caught = error;
  }
  expect(caught).toBeInstanceOf(ConnectError);
  if (!(caught instanceof ConnectError))
    throw new Error("expected ConnectError");
  expect(caught.rawMessage).toContain("models.roles.memory_formation.routes.0");
  expect(caught.rawMessage).not.toContain("absent");
  const [detail] = caught.findDetails(ErrorDetailSchema);
  expect(detail?.context).toMatchObject({
    field_path: "models.roles.memory_formation.routes.0",
    validation_code: "custom",
  });
});

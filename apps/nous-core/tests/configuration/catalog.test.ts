// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { expect, it } from "vitest";
import {
  CONFIG_REVISION,
  configurationBundle,
  coreConfigurationValues,
} from "../../src/configuration/catalog.js";
import { modelConfigurationShape } from "../../src/model/configuration.js";

it("freezes owner-normalized policies before the Kernel computes configuration identity", () => {
  const omitted = configurationBundle({
    config_revision: CONFIG_REVISION,
    video: { max_frames: 4 },
  });
  const explicit = configurationBundle({
    config_revision: CONFIG_REVISION,
    video: { ...modelConfigurationShape.video.parse(undefined), max_frames: 4 },
  });
  expect(coreConfigurationValues(omitted.deployment_document)).toEqual(
    coreConfigurationValues(explicit.deployment_document),
  );
  const paths = omitted.core_descriptors.map((descriptor) => descriptor.path);
  expect(paths).toContain("video.max_frames");
  expect(paths).toContain("audio.input_mode");
  expect(paths).toContain("material.inputs.formation_source_max_bytes");
  expect(paths).toContain("core_execution.opportunity.work_timeout_ms");
  expect(paths).not.toContain("video");
  expect(paths).not.toContain("core_execution");
});

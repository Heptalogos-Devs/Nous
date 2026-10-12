// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { MaterialInterpretation } from "./interpretation.js";
import { ModelInvocations } from "./invocations.js";
import {
  modelConfigurationSchema,
  type ModelConfiguration,
} from "./configuration.js";

export class ModelRuntime {
  readonly material: MaterialInterpretation;
  constructor(
    readonly invocations = new ModelInvocations(),
    readonly materialStrategy: ModelConfiguration["material_strategy"] = "description_only",
    readonly video = modelConfigurationSchema.parse({}).video,
    readonly credentialEnvironments: readonly string[] = [],
    readonly audio = modelConfigurationSchema.parse({}).audio,
    readonly tempRoot?: string,
    readonly materialInputs = modelConfigurationSchema.parse({})
      .material_inputs,
  ) {
    this.material = new MaterialInterpretation(invocations, video.prompt);
  }
  static async fromConfig(
    config: ModelConfiguration,
    promptRoot?: string,
    overridePromptRoot?: string,
    tempRoot?: string,
  ) {
    const invocations = await ModelInvocations.create(
      config,
      promptRoot,
      overridePromptRoot,
    );
    return new ModelRuntime(
      invocations,
      config.material_strategy,
      config.video,
      Object.values(config.gateway_profiles).map(
        (gateway) => gateway.credential_env,
      ),
      config.audio,
      tempRoot,
      config.material_inputs,
    );
  }
}

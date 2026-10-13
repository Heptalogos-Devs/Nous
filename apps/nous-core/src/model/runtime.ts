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
    readonly configuration = modelConfigurationSchema.parse({}),
    readonly tempRoot?: string,
  ) {
    this.material = new MaterialInterpretation(
      invocations,
      configuration.video.prompt,
    );
    this.credentialEnvironments = Object.values(
      configuration.gateway_profiles,
    ).map((gateway) => gateway.credential_env);
  }
  readonly credentialEnvironments: readonly string[];
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
    return new ModelRuntime(invocations, config, tempRoot);
  }
}

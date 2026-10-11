// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { defineCommand } from "citty";
import { commonArgs, stringArgs } from "./command-args.js";
import type { CliAction, CliLeaf } from "./command-types.js";

import { webSource } from "@nous-wave/client";
import type { CliEnvironment } from "../runtime.js";

function observationInput(env: CliEnvironment) {
  const { values, state, subjectId } = env;
  return {
    subjectId,
    sessionId: state.sessionId,
    requestId: crypto.randomUUID(),
    ...(values.source ? webSource(values.source) : { sourceClass: "file" }),
    observedAt: {
      seconds: BigInt(Math.floor(Date.now() / 1000)),
      nanos: 0,
    },
    admit: true,
  };
}

const observeText: CliAction = (env) =>
  env.client.cognition.observe({
    ...observationInput(env),
    material: {
      case: "inlineText",
      value: {
        text: env.required(env.values.text, "--text"),
        mediaType: "text/plain",
      },
    },
  });

const observeFile: CliAction = async (env, argument) => {
  const path = env.required(argument, "File path");
  const artifact = await env.client.artifacts.uploadFile(env.subjectId, path, {
    mediaType: env.mediaType(path),
  });
  const observation = await env.client.cognition.observe({
    ...observationInput(env),
    material: { case: "artifactId", value: artifact.artifactId },
  });
  return {
    artifact: { ...artifact, byteLength: artifact.byteLength.toString() },
    observation,
  };
};

export function createObserveCommand(leaf: CliLeaf) {
  return defineCommand({
    args: commonArgs,
    subCommands: {
      text: leaf({ ...stringArgs("text", "source") }, observeText, {
        description: "Admit bounded inline text as an Observation.",
        parameters: ["--text <text>", "--source <https URL> (optional)"],
      }),
      file: leaf({ ...stringArgs("media-type") }, observeFile, {
        description: "Upload an Artifact and admit it as an Observation.",
        parameters: ["<path>", "--media-type <mime>"],
      }),
    },
  });
}

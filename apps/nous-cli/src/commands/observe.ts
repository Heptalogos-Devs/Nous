// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { webSource } from "@nous-wave/client";
import { CliError } from "../agent.js";
import type { CliEnvironment } from "../runtime.js";
import { restoreProtocolData, clientDataSchemas } from "@nous-wave/client/data";
const { AcceptedObservationSchema, ArtifactSchema } = clientDataSchemas;

export async function observeCommands(
  env: CliEnvironment,
  action: string | undefined,
  argument?: string,
) {
  const { client, values, state, subjectId, required, mediaType } = env;
  const input = {
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
  if (action === "text")
    return client.cognition.observe({
      ...input,
      material: {
        case: "inlineText",
        value: {
          text: required(values.text, "--text"),
          mediaType: "text/plain",
        },
      },
    });
  if (action === "file") {
    const path = required(argument, "File path");
    const artifact = await client.artifacts.uploadFile(subjectId, path, {
      mediaType: mediaType(path),
    });
    const observation = await client.cognition.observe({
      ...input,
      material: { case: "artifactId", value: artifact.artifactId },
    });
    return restoreProtocolData(
      {
        artifact: restoreProtocolData(artifact, ArtifactSchema),
        ...observation,
      },
      AcceptedObservationSchema,
    );
  }

  throw new CliError("INVALID_ARGUMENT", "Unknown observe command");
}

// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { CliError } from "../agent.js";
import type { CliEnvironment } from "../runtime.js";

export async function subjectCommands(
  env: CliEnvironment,
  action: string | undefined,
  argument?: string,
) {
  const { client, save, required } = env;
  if (action === "list") return client.subjects.list({});
  if (action === "create") {
    const subject = await client.subjects.create({
      operationId: crypto.randomUUID(),
      capabilities: { memory: true },
      cognitiveSeed: {
        text: "schema_version = 1",
        format: "application/vnd.nous-wave.cognitive-seed+toml;version=1",
      },
    });
    await save({
      format: "nous.consumer.selection",
      subjectId: subject.subjectId,
    });
    return subject;
  }
  if (action === "use") {
    const subject = await client.subjects.get({
      subjectId: (
        await env.resolveReference(
          required(argument, "Subject reference"),
          "subject",
        )
      ).value,
    });
    await save({
      format: "nous.consumer.selection",
      subjectId: subject.subjectId,
    });
    return subject;
  }

  throw new CliError("INVALID_ARGUMENT", "Unknown subject command");
}

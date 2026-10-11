// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { CliError } from "../agent.js";
import type { CliEnvironment } from "../runtime.js";

export async function sessionCommands(
  env: CliEnvironment,
  action: string | undefined,
  reference?: string,
) {
  const { client, state, subjectId, save, required } = env;
  if (reference !== undefined && action !== "show")
    throw new CliError(
      "INVALID_ARGUMENT",
      "Only session show accepts an explicit reference; open and close use local selection",
    );
  if (action === "show") {
    const id =
      reference === undefined
        ? required(state.sessionId, "Selected Session (run session open)")
        : (
            await env.resolveReference(
              required(reference, "Session reference"),
              "session",
            )
          ).value;
    return client.cognition.getSession({ subjectId, id });
  }
  if (action === "open") {
    const session = await client.cognition.openSession({ subjectId });
    await save({ ...state, sessionId: session.sessionId });
    return session;
  }
  const id = required(state.sessionId, "Selected Session (run session open)");
  if (action === "close") {
    const session = await client.cognition.closeSession({ subjectId, id });
    await save({ ...state, sessionId: undefined });
    return session;
  }

  throw new CliError("INVALID_ARGUMENT", "Unknown session command");
}

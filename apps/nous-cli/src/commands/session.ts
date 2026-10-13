// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { page } from "../content.js";
import { CliError } from "../agent.js";
import type { CliEnvironment } from "../runtime.js";

export async function sessionCommands(
  env: CliEnvironment,
  action: string | undefined,
  reference?: string,
) {
  const { client, save, required } = env;
  const subjectId = await env.subjectId;
  if (action === "list")
    return client.runtime.listSessions({
      subjectId,
      status: env.values.status,
      page: page(env),
    });
  if (reference !== undefined && action !== "show")
    throw new CliError(
      "INVALID_ARGUMENT",
      "Only session show accepts an explicit reference; open and close use local selection",
    );
  if (action === "show") {
    const id =
      reference === undefined
        ? required(await env.sessionId, "Selected Session (run session open)")
        : (
            await env.resolveReference(
              required(reference, "Session reference"),
              "session",
            )
          ).value;
    return client.runtime.getSession({ subjectId, id });
  }
  if (action === "open") {
    const state = await env.state;
    const session = await client.runtime.openSession({ subjectId });
    await save({ ...state, sessionId: session.sessionId });
    return session;
  }
  const state = await env.state;
  const id = required(
    await env.sessionId,
    "Selected Session (run session open)",
  );
  if (action === "close") {
    const session = await client.runtime.closeSession({ subjectId, id });
    await save({ ...state, sessionId: undefined });
    return session;
  }

  throw new CliError("INVALID_ARGUMENT", "Unknown session command");
}

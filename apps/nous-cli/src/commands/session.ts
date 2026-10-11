// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { defineCommand } from "citty";
import { commonArgs } from "./command-args.js";
import type { CliLeaf } from "./command-types.js";

import { CliError } from "../agent.js";
import type { CliEnvironment } from "../runtime.js";

const onlyShowAcceptsReference = (reference?: string) => {
  if (reference !== undefined)
    throw new CliError(
      "INVALID_ARGUMENT",
      "Only session show accepts an explicit reference; open and close use local selection",
    );
};

const sessionOpen = async (env: CliEnvironment, reference?: string) => {
  onlyShowAcceptsReference(reference);
  const { client, state, subjectId, save } = env;
  const session = await client.cognition.openSession({ subjectId });
  await save({ ...state, sessionId: session.sessionId });
  return session;
};
const sessionShow = async (env: CliEnvironment, reference?: string) => {
  const { client, state, subjectId, required } = env;
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
};
const sessionClose = async (env: CliEnvironment, reference?: string) => {
  onlyShowAcceptsReference(reference);
  const { client, state, subjectId, save, required } = env;
  const id = required(state.sessionId, "Selected Session (run session open)");
  const session = await client.cognition.closeSession({ subjectId, id });
  await save({ ...state, sessionId: undefined });
  return session;
};

export function createSessionCommand(leaf: CliLeaf) {
  return defineCommand({
    args: commonArgs,
    subCommands: {
      open: leaf({}, sessionOpen),
      show: leaf({}, sessionShow, {
        description: "Read a Session without opening or selecting it.",
        parameters: ["[Session reference]"],
      }),
      close: leaf({}, sessionClose),
    },
  });
}

// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { MemoryContentSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { lifecycle, revisionLink } from "../cognition-owner.js";
import { content, objectEpoch, operationId, page, target } from "../content.js";
import type { CliEnvironment } from "../runtime.js";
export async function memoryCommands(
  env: CliEnvironment,
  action?: string,
  ref?: string,
) {
  const { client, subjectId, values } = env;
  if (action === "list")
    return client.memory.listMemories({
      subjectId,
      status: env.values.status,
      page: page(env),
    });
  if (action === "form")
    return client.memory.formMemory({
      subjectId,
      operationId: operationId(env),
      cognitiveRole: env.required(
        values.kind,
        "--kind experiential|declarative|procedural_experience",
      ),
      input: await content(env, MemoryContentSchema),
    });
  if (action === "revision")
    return client.memory.getMemoryRevision({
      subjectId,
      id: await target(env, ref, "memory_revision"),
    });
  if (action === "link")
    return client.memory.linkRevisions(
      await revisionLink(env, "memory_revision"),
    );
  const memoryId = await target(env, ref, "memory");
  if (action === "get")
    return client.memory.getMemory({ subjectId, id: memoryId });
  if (action === "history")
    return client.memory.listMemoryRevisions({
      subjectId,
      memoryId,
      page: page(env),
    });
  const mutation = {
    subjectId,
    memoryId,
    expectedObjectEpoch:
      objectEpoch(values["expected-object-epoch"]) ??
      (await client.memory.getMemory({ subjectId, id: memoryId })).objectEpoch,
    operationId: operationId(env),
  };
  if (action === "revise")
    return client.memory.reviseMemory({
      ...mutation,
      intent: env.required(values.kind, "--kind correct|rephrase|reinterpret"),
      input: await content(env, MemoryContentSchema),
    });
  if (action === "accessibility")
    return client.memory.setAccessibility({
      ...mutation,
      mode: env.required(values.kind, "--kind auto|normal|deep|explicit"),
    });
  return lifecycle(action, mutation, {
    suppress: client.memory.suppressMemory,
    restore: client.memory.restoreMemory,
    withdraw: client.memory.withdrawMemory,
    reaccept: client.memory.reacceptMemory,
    purge: client.memory.purgeMemory,
  });
}

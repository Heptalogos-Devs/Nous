// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { EpisodeContentSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { lifecycle, revisionLink } from "../cognition-owner.js";
import { CliError } from "../agent.js";
import { content, objectEpoch, operationId, page, target } from "../content.js";
import type { CliEnvironment } from "../runtime.js";
export async function episodeCommands(
  env: CliEnvironment,
  action?: string,
  ref?: string,
) {
  const { client } = env;
  const subjectId = await env.subjectId;
  if (action === "list")
    return client.memory.listEpisodes({
      subjectId,
      status: env.values.status,
      page: page(env),
    });
  if (action === "create")
    return client.memory.createEpisode({
      content: await content(env, EpisodeContentSchema),
      trackKey: env.required(env.values.kind, "--kind <segmentation track>"),
      subjectId,
      operationId: operationId(env),
    });
  if (action === "revision")
    return client.memory.getEpisodeRevision({
      subjectId,
      id: await target(env, ref, "episode_revision"),
    });
  if (action === "link")
    return client.memory.linkEpisodeRevisions(
      await revisionLink(env, "episode_revision"),
    );
  const episodeId = await target(env, ref, "episode");
  if (action === "history")
    return client.memory.listEpisodeRevisions({
      subjectId,
      episodeId,
      page: page(env),
    });
  if (action === "get")
    return client.memory.getEpisode({ subjectId, id: episodeId });
  let expectedObjectEpoch = objectEpoch(env.values["expected-object-epoch"]);
  if (expectedObjectEpoch === undefined) {
    const current = await client.memory.getEpisode({
      subjectId,
      id: episodeId,
    });
    if (!current.episode) throw new CliError("NOT_FOUND", "Episode not found");
    expectedObjectEpoch = current.episode.objectEpoch;
  }
  const mutation = {
    subjectId,
    episodeId,
    expectedObjectEpoch,
    operationId: operationId(env),
  };
  if (action === "revise")
    return client.memory.reviseEpisode({
      content: await content(env, EpisodeContentSchema),
      ...mutation,
      intent: env.required(env.values.kind, "--kind resegment|reinterpret"),
    });
  return lifecycle(action, mutation, {
    suppress: client.memory.suppressEpisode,
    restore: client.memory.restoreEpisode,
    withdraw: client.memory.withdrawEpisode,
    reaccept: client.memory.reacceptEpisode,
    purge: client.memory.purgeEpisode,
  });
}
export async function journalCommands(
  env: CliEnvironment,
  action?: string,
  ref?: string,
) {
  const { client } = env;
  const subjectId = await env.subjectId;
  if (action === "list")
    return client.memory.listJournals({
      subjectId,
      status: env.values.status,
      page: page(env),
    });
  if (action === "revision")
    return client.memory.getJournalRevision({
      subjectId,
      id: await target(env, ref, "journal_revision"),
    });
  const journalId = await target(env, ref, "journal");
  if (action === "history")
    return client.memory.listJournalRevisions({
      subjectId,
      journalId,
      page: page(env),
    });
  if (action === "get")
    return client.memory.getJournal({ subjectId, id: journalId });
  let expectedObjectEpoch = objectEpoch(env.values["expected-object-epoch"]);
  if (expectedObjectEpoch === undefined) {
    const current = await client.memory.getJournal({
      subjectId,
      id: journalId,
    });
    if (!current.journal) throw new CliError("NOT_FOUND", "Journal not found");
    expectedObjectEpoch = current.journal.objectEpoch;
  }
  const mutation = {
    subjectId,
    journalId,
    expectedObjectEpoch,
    operationId: operationId(env),
  };
  return lifecycle(action, mutation, {
    suppress: client.memory.suppressJournal,
    restore: client.memory.restoreJournal,
    withdraw: client.memory.withdrawJournal,
    reaccept: client.memory.reacceptJournal,
    purge: client.memory.purgeJournal,
  });
}

// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { CliError, boundedInteger } from "../agent.js";
import type { CliEnvironment } from "../runtime.js";
import { pipeline } from "node:stream/promises";
import { open, unlink, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
const cognitionKinds = new Set([
  "memory_revision",
  "cognitive_schema_revision",
  "episode_revision",
  "journal_revision",
]);
export async function queryCommands(
  env: CliEnvironment,
  action?: string,
  argument?: string,
  positionals: string[] = [],
) {
  const { client, values, required, save } = env;
  const subjectId = await env.subjectId;
  const state = await env.state;
  const preparing = action === "prepare" || action === "inspect";
  const nousql = values["query-file"]
    ? await env.readText(values["query-file"], 32768)
    : required(
        positionals.length
          ? positionals.slice(preparing ? 2 : 1).join(" ")
          : preparing
            ? argument
            : action,
        "NousQL",
      );
  if (Buffer.byteLength(nousql) > 32768)
    throw new CliError("INVALID_ARGUMENT", "NousQL exceeds 32 KiB");
  const request = {
    subjectId,
    sessionId: await env.sessionId,
    workContextId: await env.workContextId,
    nousql,
  };
  if (preparing) {
    const response = await client.cognition.prepareQuery(request);
    const { $typeName: _type, $unknown: _unknown, ...inspection } = response;
    return {
      ...inspection,
      boundQuery: JSON.parse(response.boundQuery) as unknown,
    };
  }
  // A failed new query must not leave result:N pointing at an unrelated old answer.
  await save({
    ...state,
    sessionId: request.sessionId,
    workContextId: request.workContextId,
    lastQuery: undefined,
  });
  const response = await client.cognition.query(request);
  const refs = response.hits.map((hit) => hit.revision ?? hit.reference);
  if (refs.some((ref) => !ref))
    throw new CliError(
      "REFERENCE_TYPE_MISMATCH",
      "Query result has no reference",
    );
  await save({
    ...state,
    sessionId: request.sessionId,
    workContextId: request.workContextId,
    lastQuery: {
      subjectId,
      queryId: response.queryId,
      results: refs.map((ref) => ({ kind: ref!.kind, value: ref!.value })),
    },
  });
  if (values.raw) return response;
  return {
    queryId: response.queryId,
    queryRef: "query:last",
    status: response.status,
    results: response.hits.map((hit, index) => ({
      lexicalRef: hit.lexicalRef,
      result: `result:${index + 1}`,
      ref: refs[index],
      text: hit.text,
      authority: hit.authority,
      cognitiveRole: hit.cognitiveRole,
      formationMode: hit.formationMode,
      evidenceFamilies: hit.evidenceFamilies,
      next: cognitionKinds.has(refs[index]!.kind)
        ? `show result:${index + 1} | trace result:${index + 1} | use result:${index + 1} | context pin --cognition result:${index + 1}`
        : refs[index]!.kind === "occurrence"
          ? `show result:${index + 1} | read result:${index + 1} | context pin --cognition result:${index + 1}`
          : refs[index]!.kind === "resource"
            ? `show result:${index + 1}`
            : [
                  "artifact",
                  "source_region",
                  "derived_representation",
                  "derived_region",
                ].includes(refs[index]!.kind)
              ? `show result:${index + 1} | read result:${index + 1}`
              : "Returned source reference; cognition use and pin are unavailable",
    })),
    resourceRecords: response.resourceRecords,
    resourceActions: response.resourceActions,
    diagnostics:
      response.diagnostics &&
      (values.developer
        ? {
            ...response.diagnostics,
            ...(response.diagnostics.trace
              ? { trace: JSON.parse(response.diagnostics.trace) as unknown }
              : {}),
          }
        : {
            candidateCounts: response.diagnostics.candidateCounts,
            laneStatus: response.diagnostics.laneStatus,
            topologyComplete: response.diagnostics.topologyComplete,
            topologyDiscardedMass: response.diagnostics.topologyDiscardedMass,
          }),
    degradation: response.degradation,
  };
}
export async function showCommands(env: CliEnvironment, reference?: string) {
  const { client, required, resolveReference } = env;
  const subjectId = await env.subjectId;
  const ref = await resolveReference(
    required(reference, "Exact reference or result:N"),
  );
  const input = { subjectId, id: ref.value };
  switch (ref.kind) {
    case "memory_revision":
      return client.memory.getMemoryRevision(input);
    case "cognitive_schema_revision":
      return client.concepts.getCognitiveSchemaRevision(input);
    case "episode_revision":
      return client.memory.getEpisodeRevision(input);
    case "journal_revision":
      return client.memory.getJournalRevision(input);
    case "occurrence":
      return client.material.getOccurrence(input);
    case "artifact": {
      const artifact = await client.material.getArtifact(input);
      const observations = await client.material.listOccurrences({
        subjectId,
        artifactId: artifact.artifactId,
        limit: 20,
      });
      return { ...artifact, observations };
    }
    case "source_region": {
      const region = await client.material.getSourceRegion(input);
      const observations = await client.material.listOccurrences({
        subjectId,
        artifactId: region.artifactId,
        limit: 20,
      });
      return { ...region, observations };
    }
    case "derived_representation":
      return client.material.getDerivedRepresentation(input);
    case "derived_region":
      return client.material.getDerivedRegion(input);
    case "resource":
      return client.resourceRegistry.getResource(input);
    default:
      throw new CliError(
        "REFERENCE_TYPE_MISMATCH",
        `show is unavailable for ${ref.kind}; use the returned source details`,
      );
  }
}

export async function readCommands(env: CliEnvironment, reference?: string) {
  const ref = await env.resolveReference(
    env.required(reference, "Material reference or result:N"),
  );
  if (
    ![
      "occurrence",
      "artifact",
      "source_region",
      "derived_representation",
      "derived_region",
    ].includes(ref.kind)
  )
    throw new CliError(
      "REFERENCE_TYPE_MISMATCH",
      "read requires a Material reference; show reads cognition revisions",
    );
  if (ref.kind === "artifact" && env.values.output) {
    const artifact = await env.client.material.getArtifact({
      subjectId: await env.subjectId,
      id: ref.value,
    });
    const outputPath = resolve(env.values.output);
    const file = await open(outputPath, "wx");
    let failed = false;
    try {
      const source = await env.client.artifacts.download(
        await env.subjectId,
        ref.value,
      );
      await pipeline(source.content, file.createWriteStream());
    } catch (error) {
      failed = true;
      throw error;
    } finally {
      await file.close();
      if (failed) await unlink(outputPath);
    }
    return {
      reference: ref,
      mediaType: artifact.mediaType,
      totalBytes: artifact.byteLength,
      outputPath,
      partial: false,
    };
  }
  const material = await env.client.material.materializeEvidence({
    subjectId: await env.subjectId,
    reference: ref,
    maxBytes: BigInt(
      boundedInteger(
        env.values["max-bytes"] ?? "65536",
        1,
        1048576,
        "--max-bytes",
      ),
    ),
  });
  const textLike = /^text\/|^application\/(?:json|xml|.*\+json)(?:;|$)/.test(
    material.mediaType,
  );
  const outputPath = env.values.output && resolve(env.values.output);
  if (outputPath) {
    if (material.partial)
      throw new CliError(
        "INVALID_ARGUMENT",
        "Source is partial; increase --max-bytes or select a bounded region before exporting",
      );
    await writeFile(outputPath, material.content, { flag: "wx" });
  }
  return {
    reference: material.reference,
    mediaType: material.mediaType,
    totalBytes: material.totalBytes,
    rangeStart: material.rangeStart,
    rangeEnd: material.rangeEnd,
    partial: material.partial,
    ...(outputPath ? { outputPath } : {}),
    ...(textLike
      ? {
          text: new TextDecoder("utf-8", { fatal: true }).decode(
            material.content,
            { stream: material.partial },
          ),
        }
      : {
          next: "Inspect original bytes with read <reference> --max-bytes <bound> --output <new-file>; derive <src: reference> creates a model interpretation.",
        }),
    evidence: material.evidence,
    degradation: material.degradation,
  };
}

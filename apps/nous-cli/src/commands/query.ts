// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { CliError, boundedInteger } from "../agent.js";
import type { CliEnvironment } from "../runtime.js";
import { writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import { toJson } from "@bufbuild/protobuf";
import { QueryDiagnosticsSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { isCognitionRevisionReference, typedCognition } from "./cognition.js";
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
  const { client, state, subjectId, values, required, save } = env;
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
    sessionId: state.sessionId,
    workContextId: state.workContextId,
    nousql,
  };
  if (preparing) {
    const response = await client.cognition.prepareQuery(request);
    if (values.raw) return response;
    const bound = response.boundQuery;
    if (!bound)
      throw new CliError(
        "INTERNAL_FAILURE",
        "Core omitted the typed query inspection",
      );
    const context = bound.context;
    const workContext = context?.workContext;
    const session = context?.session;
    const activation = bound.activation;
    return {
      intent: nousql,
      queryId: bound.queryId,
      inspection: bound,
      representation: {
        sha256: bound.representation?.sha256,
        chars: Array.from(response.embeddingText).length,
        truncationFlags: bound.representation?.truncationFlags ?? [],
      },
      context: {
        digest: context?.digest,
        workContext: workContext && {
          workContextId: workContext.workContextId,
          revision: workContext.revision.toString(),
          purpose: workContext.purpose,
          cognitionAnchors: workContext.cognitionAnchors,
          entityAnchors: workContext.entityAnchors,
          tagAnchors: workContext.tagAnchors,
        },
        session: session && {
          sessionId: session.sessionId,
          runtimeRevision: session.runtimeRevision.toString(),
          residentCount: session.residentRefs.length,
        },
        historyExclusions: context?.historyExclusions,
      },
      activation: {
        queryAndContextTags: activation?.explicitTags.flatMap((value) =>
          value.tag ? [value.tag] : [],
        ),
        inferredTags: activation?.inferredTags.flatMap((value) =>
          value.tag ? [value.tag] : [],
        ),
        modelCalls: activation?.modelCalls,
      },
      projection: bound.projection,
      temporalFrame: bound.readContext?.temporal,
      capabilities: {
        textEmbedding: response.textEmbeddingRequirement,
        rerank: response.rerankRequirement,
        conceptEnrichment: response.conceptEnrichmentRequirement,
        conceptEnrichmentMode: response.conceptEnrichmentMode,
      },
      embeddingRequired: response.embeddingRequired,
      degradation: activation?.degradation,
    };
  }
  // A failed new query must not leave result:N pointing at an unrelated old answer.
  await save({ ...state, lastQuery: undefined });
  const response = await client.cognition.query(request);
  const diagnostics = response.diagnostics
    ? (toJson(QueryDiagnosticsSchema, response.diagnostics) as Record<
        string,
        unknown
      >)
    : undefined;
  const refs = response.hits.map((hit) => hit.revision ?? hit.reference);
  if (refs.some((ref) => !ref))
    throw new CliError(
      "REFERENCE_TYPE_MISMATCH",
      "Query result has no reference",
    );
  await save({
    ...state,
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
      diagnostics &&
      (values.developer
        ? {
            ...diagnostics,
            ...(response.diagnostics?.trace
              ? { trace: JSON.parse(response.diagnostics.trace) as unknown }
              : {}),
          }
        : {
            candidateCounts: diagnostics.candidateCounts,
            laneStatus: diagnostics.laneStatus,
            topologyComplete: diagnostics.topologyComplete,
            topologyDiscardedMass: diagnostics.topologyDiscardedMass,
          }),
    degradation: response.degradation,
  };
}
export async function showCommands(env: CliEnvironment, reference?: string) {
  const { client, subjectId, required, resolveReference } = env;
  const ref = await resolveReference(
    required(reference, "Exact reference or result:N"),
  );
  if (isCognitionRevisionReference(ref)) return typedCognition.read(env, ref);
  const input = { subjectId, id: ref.value };
  switch (ref.kind) {
    case "occurrence":
      return client.material.occurrence(input);
    case "artifact": {
      const artifact = await client.material.getArtifact(input);
      const observations = await client.material.occurrences({
        subjectId,
        artifactId: artifact.artifactId,
        limit: 20,
      });
      return { ...artifact, observations };
    }
    case "source_region": {
      const region = await client.material.sourceRegion(input);
      const observations = await client.material.occurrences({
        subjectId,
        artifactId: region.artifactId,
        limit: 20,
      });
      return { ...region, observations };
    }
    case "derived_representation":
      return client.material.representation(input);
    case "derived_region":
      return client.material.derivedRegion(input);
    case "resource":
      return client.resources.get(input);
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
  const material = await env.client.material.materialize({
    subjectId: env.subjectId,
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
    totalBytes: material.totalBytes.toString(),
    rangeStart: material.rangeStart.toString(),
    rangeEnd: material.rangeEnd.toString(),
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

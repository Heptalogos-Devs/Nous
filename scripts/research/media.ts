// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { workspacePaths } from "../workspace.js";
import type { connectNousInstance } from "@nous-wave/client/node";
import { readFile, writeFile, mkdir } from "node:fs/promises";
import { dirname, resolve, sep } from "node:path";
import { pathToFileURL } from "node:url";
import { randomUUID, createHash } from "node:crypto";
import { create } from "@bufbuild/protobuf";
import { CreateSubjectRequestSchema } from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import { parseArgs } from "node:util";
import { performance } from "node:perf_hooks";
import { setTimeout as delay } from "node:timers/promises";

type Client = Awaited<ReturnType<typeof connectNousInstance>>;
type Unit = {
  id: string;
  file: string;
  sha256: string;
  media_type: string;
  source_url: string;
  rights: string;
  context?: Record<string, string>;
  query: string;
  oracle_patterns: string[];
  oracle_patterns_by_representation_kind?: Record<string, string[]>;
  oracle_status: "PASS" | "NOT_RUN";
};
type Receipt = {
  requestId: string;
  createdAtMs: number;
  artifactId?: string;
  occurrenceId?: string;
  sourceRegionId?: string;
  strategies: Record<
    string,
    { operationId: string; revisionId?: string; result?: unknown }
  >;
  query?: { status: "PASS" | "FAIL"; response: unknown };
};
const { values } = parseArgs({
  options: {
    "run-root": { type: "string" },
    "client-module": { type: "string" },
    manifest: { type: "string", default: "docs/research/corpus/media.json" },
    unit: { type: "string", multiple: true },
    strategy: { type: "string", multiple: true },
    "derive-only": { type: "boolean", default: false },
    "allow-degradation": { type: "string", multiple: true },
    "skip-retrieval": { type: "boolean", default: false },
    "raw-root": {
      type: "string",
      default: resolve(workspacePaths.research, "corpus/raw"),
    },
    state: {
      type: "string",
      default: resolve(workspacePaths.research, "runs/media-state.json"),
    },
  },
});
if (!values["run-root"] || !values["client-module"])
  throw new Error("--run-root and distributed --client-module required");
const module = (await import(
  pathToFileURL(resolve(values["client-module"])).href
)) as { connectNousInstance: typeof connectNousInstance };
const client: Client = await module.connectNousInstance({
  runRoot: resolve(values["run-root"]),
});
const cancellation = new AbortController();
process.once("SIGINT", () =>
  cancellation.abort(new Error("Research interrupted")),
);
const options = { timeoutMs: 180000, signal: cancellation.signal };
const manifestBytes = await readFile(values.manifest),
  digest = createHash("sha256").update(manifestBytes).digest("hex");
const allUnits = (
  JSON.parse(manifestBytes.toString("utf8")) as { units: Unit[] }
).units;
const units = values.unit
  ? allUnits.filter((unit) => values.unit!.includes(unit.id))
  : allUnits;
if (values.unit?.some((id) => !allUnits.some((unit) => unit.id === id)))
  throw new Error("Unknown media unit");
const strategies = values.strategy ?? [
  "description_only",
  "direct_structured",
  "describe_then_structure",
];
if (
  strategies.some(
    (strategy) =>
      ![
        "description_only",
        "direct_structured",
        "describe_then_structure",
      ].includes(strategy),
  )
)
  throw new Error("Unknown media strategy");
const statePath = resolve(values.state);
const state = JSON.parse(
  await readFile(statePath, "utf8").catch((error: NodeJS.ErrnoException) => {
    if (error.code === "ENOENT")
      return JSON.stringify({
        manifestDigest: digest,
        createId: randomUUID(),
        units: {},
      });
    throw error;
  }),
) as {
  manifestDigest: string;
  createId: string;
  subjectId?: string;
  units: Record<string, Receipt>;
};
if (state.manifestDigest !== digest)
  throw new Error("Media manifest changed; select a new state");
const json = (value: unknown) =>
  JSON.stringify(
    value,
    (_, item: unknown) => (typeof item === "bigint" ? item.toString() : item),
    2,
  );
const save = async () => {
  await mkdir(dirname(statePath), { recursive: true });
  await writeFile(statePath, json(state));
};
if (!state.subjectId) {
  state.subjectId = randomUUID();
  await save();
  const subject = await client.subjects.create(
    create(CreateSubjectRequestSchema, {
      subjectId: state.subjectId,
      operationId: state.createId,
      capabilities: { memory: true },
      cognitiveSeed: {
        text: "schema_version = 1",
        format: "application/vnd.nous-wave.cognitive-seed+toml;version=1",
      },
    }),
    options,
  );
  if (subject.subjectId !== state.subjectId)
    throw new Error("Subject create returned another frozen Subject ID");
  await save();
}
const subjectId = state.subjectId;
const rawRoot = resolve(values["raw-root"]);
for (const unit of units) {
  const file = resolve(rawRoot, unit.file);
  if (!file.startsWith(rawRoot + sep))
    throw new Error("Media cache locator escapes raw root");
  if (
    createHash("sha256")
      .update(await readFile(file))
      .digest("hex") !== unit.sha256
  )
    throw new Error(`Media digest changed: ${unit.id}`);
  const receipt = (state.units[unit.id] ??= {
    requestId: randomUUID(),
    createdAtMs: Date.now(),
    strategies: {},
  });
  await save();
  if (!receipt.artifactId) {
    const artifact = await client.artifacts.uploadFile(subjectId, file, {
      ...options,
      mediaType: unit.media_type,
    });
    receipt.artifactId = artifact.artifactId;
    await save();
  }
  if (!receipt.occurrenceId) {
    const observed = await client.cognition.observe(
      {
        subjectId,
        requestId: receipt.requestId,
        sourceClass: "web",
        context: {
          ...unit.context,
          source_url: unit.source_url,
          source_sha256: unit.sha256,
          rights: unit.rights,
        },
        observedAt: { seconds: BigInt(Math.floor(receipt.createdAtMs / 1000)) },
        admit: true,
        material: { case: "artifactId", value: receipt.artifactId },
      },
      options,
    );
    receipt.occurrenceId = observed.occurrenceId;
    receipt.sourceRegionId = observed.sourceRegionId;
    await save();
  }
  for (const strategy of strategies) {
    const operation = (receipt.strategies[strategy] ??= {
      operationId: randomUUID(),
    });
    if (
      (values["derive-only"] || operation.revisionId) &&
      (operation.result as { status?: string } | undefined)?.status === "PASS"
    )
      continue;
    await save();
    const start = performance.now();
    let partialDerivation: unknown;
    try {
      const derived = await client.model.deriveMaterial(
        { subjectId, sourceRegionId: receipt.sourceRegionId!, strategy },
        options,
      );
      partialDerivation = derived;
      const selected = derived.representations.find(
        (item) => item.representationId === derived.selectedRepresentationId,
      );
      const unexpectedDegradation = derived.degradation.filter(
        (item) => !values["allow-degradation"]?.includes(item.code),
      );
      if (!selected || unexpectedDegradation.length)
        throw new Error(`Derivation incomplete: ${json(derived.degradation)}`);
      if (strategy !== "description_only" && !selected.structuredPayload)
        throw new Error("Structured strategy did not commit a JSON payload");
      const formed = values["derive-only"]
        ? undefined
        : await client.model.formFromObservation(
            {
              subjectId,
              operationId: operation.operationId,
              occurrenceId: receipt.occurrenceId,
              representationId: selected.representationId,
              aboutnessMode: "none",
            },
            options,
          );
      if (formed && !formed.memory)
        throw new Error(
          `Formation did not commit Memory: ${json(formed.degradation)}`,
        );
      if (formed?.memory) operation.revisionId = formed.memory.revisionId;
      const oraclePatterns =
        unit.oracle_patterns_by_representation_kind?.[selected.kind] ??
        unit.oracle_patterns;
      const missingFacts = oraclePatterns.filter(
        (pattern) => !new RegExp(pattern, "is").test(selected.text ?? ""),
      );
      const roots = new Set<string>(),
        visited = new Set<string>();
      const trace = async (reference: {
        kind: string;
        value: string;
      }): Promise<void> => {
        const key = `${reference.kind}:${reference.value}`;
        if (visited.has(key)) return;
        if (visited.size >= 512)
          throw new Error("Provenance trace bound exhausted");
        visited.add(key);
        if (reference.kind === "derived_representation") {
          const representation = await client.material.representation(
            { subjectId, id: reference.value },
            options,
          );
          for (const input of representation.inputs)
            if (input.reference) await trace(input.reference);
        } else if (reference.kind === "source_region") {
          const region = await client.material.sourceRegion(
            { subjectId, id: reference.value },
            options,
          );
          roots.add(region.artifactId);
        } else if (reference.kind === "derived_region") {
          const region = await client.material.derivedRegion(
            { subjectId, id: reference.value },
            options,
          );
          await trace({
            kind: "derived_representation",
            value: region.representationId,
          });
        }
      };
      await trace({
        kind: "derived_representation",
        value: selected.representationId,
      });
      if (roots.size !== 1 || !roots.has(receipt.artifactId!))
        throw new Error("Provenance does not reach the uploaded source");
      operation.result = {
        status: "PASS",
        quality:
          unit.oracle_status === "NOT_RUN"
            ? "NOT_RUN"
            : missingFacts.length
              ? "FAIL"
              : "PASS",
        missingFacts,
        strategy,
        latencyMs: performance.now() - start,
        derived,
        formation: formed,
        sourceRoots: [...roots],
        traceNodes: visited.size,
      };
    } catch (error) {
      operation.result = {
        status: "FAIL",
        strategy,
        derived: partialDerivation,
        error: error instanceof Error ? error.message : "Operation failed",
      };
    }
    await save();
    process.stdout.write(
      json({
        unit: unit.id,
        strategy,
        result: (operation.result as { status: string }).status,
      }) + "\n",
    );
  }
}
if (!values["derive-only"] && !values["skip-retrieval"]) {
  while (true) {
    const batch = await client.model.prepareEmbeddings(
      { subjectId, limit: 1 },
      options,
    );
    await save();
    if (batch.degradation.length)
      throw new Error(
        `Media embeddings incomplete: ${json(batch.degradation)}`,
      );
    if (!batch.committed) break;
    await delay(2000, undefined, { signal: cancellation.signal });
  }
  for (const unit of units) {
    const response = await client.cognition.query(
      {
        subjectId,
        nousql: `${JSON.stringify(unit.query)} $return(memory) $limit(10) $diagnostics(full)`,
      },
      options,
    );
    const expected = Object.values(state.units[unit.id]!.strategies)
      .map((operation) => operation.revisionId)
      .filter(Boolean);
    const recalled = response.hits.filter((hit) =>
      expected.includes(hit.reference?.value),
    );
    state.units[unit.id]!.query = {
      status:
        recalled.length && response.status === "complete" ? "PASS" : "FAIL",
      response,
    };
    await save();
  }
}
const outcomes = units
  .map((unit) => state.units[unit.id]!)
  .flatMap((unit) => strategies.map((strategy) => unit.strategies[strategy]!))
  .map(
    (operation) =>
      operation.result as { status?: string; quality?: string } | undefined,
  );
const failed = outcomes.filter((outcome) => outcome?.status !== "PASS").length;
const qualityFailures = outcomes.filter(
  (outcome) => outcome?.quality === "FAIL",
).length;
const unverified = outcomes.filter(
  (outcome) => outcome?.quality === "NOT_RUN",
).length;
const queryFailures =
  values["derive-only"] || values["skip-retrieval"]
    ? 0
    : units.filter((unit) => state.units[unit.id]?.query?.status !== "PASS")
        .length;
process.stdout.write(
  json({
    status:
      failed || qualityFailures || queryFailures
        ? "FAIL"
        : unverified
          ? "NOT_RUN"
          : "PASS",
    pipelineStatus: failed ? "FAIL" : "PASS",
    qualityStatus: qualityFailures ? "FAIL" : unverified ? "NOT_RUN" : "PASS",
    queryStatus:
      values["derive-only"] || values["skip-retrieval"]
        ? "NOT_RUN"
        : queryFailures
          ? "FAIL"
          : "PASS",
    subjectId,
    units: units.length,
    failed,
    qualityFailures,
    unverified,
    queryFailures,
  }) + "\n",
);
if (failed || qualityFailures || queryFailures) process.exitCode = 1;

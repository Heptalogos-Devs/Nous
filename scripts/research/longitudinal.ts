// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { parseArgs } from "node:util";
import { readFile, writeFile, mkdir } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fromJson, toJson, type JsonValue } from "@bufbuild/protobuf";
import { MaintenancePlanSchema } from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/longitudinal_pb.js";
import { coreConfigurationValues } from "../../apps/nous-core/src/configuration/catalog.js";
import {
  parseConfiguration,
  parseEffectiveConfiguration,
} from "../../apps/nous-core/src/configuration/schema.js";
import { ModelRuntime } from "../../apps/nous-core/src/model/runtime.js";
import { structuredContractForRole } from "../../apps/nous-core/src/model/schemas/contracts.js";
import type { ModelRole } from "../../apps/nous-core/src/model/roles.js";
import { canonicalDigest } from "../../apps/nous-core/src/digest.js";
import {
  episodePartitionSchema,
  journalSynthesisSchema,
  partitionIndices,
  journalBasisKeys,
} from "../../apps/nous-core/src/model/schemas/longitudinal.js";
import { consolidationSchema } from "../../apps/nous-core/src/model/schemas/consolidation.js";

const { values } = parseArgs({
  options: {
    config: { type: "string" },
    input: { type: "string" },
    invocation: { type: "string" },
    output: { type: "string" },
    role: { type: "string" },
    "prompt-root": { type: "string", default: "prompts" },
    "override-prompt-root": { type: "string" },
    help: { type: "boolean", default: false },
  },
});
if (values.help) {
  console.log(
    "pnpm research:longitudinal --config <nous.toml> (--input <MaintenancePlan.json> | --invocation <captured-request.json>) --role <cognitive-role> --output <local-result.json> [--prompt-root prompts] [--override-prompt-root directory]. Captured invocation replays only its input through the current role/Prompt/contract; it does not commit Authority.",
  );
} else {
  await run();
}
async function run() {
  const role = values.role as ModelRole | undefined;
  if (
    !values.config ||
    (!values.input && !values.invocation) ||
    (values.input && values.invocation) ||
    !values.output ||
    ![
      "episode_segmentation",
      "journal_synthesis",
      "memory_consolidation",
      "memory_formation",
      "concept_maintenance",
      "query_concept_enrichment",
    ].includes(role ?? "")
  )
    throw new Error(
      "config, input, supported role and output are required; see --help",
    );
  const bytes = await readFile(resolve(values.input ?? values.invocation!));
  if (bytes.length > 262144)
    throw new Error("MaintenancePlan input exceeds 256 KiB");
  const captured = values.invocation
    ? (JSON.parse(bytes.toString("utf8")) as {
        messages: { role: string; content: string }[];
        response_format: { json_schema: { name: string } };
      })
    : undefined;
  if (
    captured &&
    (captured.messages.length !== 2 ||
      captured.messages[0]?.role !== "system" ||
      captured.messages[1]?.role !== "user" ||
      typeof captured.messages[1]?.content !== "string" ||
      captured.response_format?.json_schema?.name !==
        structuredContractForRole(role!)?.providerName)
  )
    throw new Error(
      "Captured invocation must match the selected role and have one exact text input",
    );
  const plan = captured
    ? undefined
    : fromJson(
        MaintenancePlanSchema,
        JSON.parse(bytes.toString("utf8")) as JsonValue,
      );
  if (
    plan &&
    ![
      "episode_segmentation",
      "journal_synthesis",
      "memory_consolidation",
    ].includes(role!)
  )
    throw new Error("This role requires a captured invocation input");
  if (
    plan &&
    (plan.status !== "ready" ||
      !plan.subjectId ||
      plan.members.length > 2048 ||
      plan.basis.length > 512 ||
      plan.candidates.length > 32)
  )
    throw new Error(
      "Input must be a ready bounded MaintenancePlan with an exact Subject/source catalog",
    );
  const { document } = parseConfiguration(
    await readFile(resolve(values.config), "utf8"),
    true,
  );
  const { models: configuration } = parseEffectiveConfiguration(
    coreConfigurationValues(document),
  );
  const models = await ModelRuntime.fromConfig(
    configuration,
    resolve(values["prompt-root"]!),
    values["override-prompt-root"]
      ? resolve(values["override-prompt-root"])
      : undefined,
  );
  const signal = new AbortController();
  process.once("SIGINT", () => signal.abort(new Error("Research interrupted")));
  const input =
    captured?.messages[1]!.content ??
    JSON.stringify(toJson(MaintenancePlanSchema, plan!));
  const snapshot = models.invocations.snapshot(role!);
  let result;
  if (captured) {
    result = await models.invocations.generate(
      role!,
      { content: input },
      { signal: signal.signal, snapshot: snapshot },
    );
  } else if (role === "episode_segmentation") {
    result = await models.segmentEpisode(input, signal.signal, snapshot);
    partitionIndices(
      episodePartitionSchema.parse(result.value),
      plan!.members.map((member) => member.key),
    );
  } else if (role === "journal_synthesis") {
    result = await models.synthesizeJournal(input, signal.signal, snapshot);
    journalBasisKeys(
      journalSynthesisSchema.parse(result.value),
      new Set(plan!.basis.map((basis) => basis.key)),
    );
  } else {
    result = await models.consolidate(input, signal.signal, snapshot);
    consolidationSchema.parse(result.value);
  }
  await mkdir(dirname(resolve(values.output)), { recursive: true });
  await writeFile(
    resolve(values.output),
    JSON.stringify(
      {
        role,
        subjectId: plan?.subjectId,
        mode: captured
          ? "pure_model_regeneration"
          : "maintenance_plan_proposal",
        inputDigest: canonicalDigest(input),
        sourceInvocationDigest: captured
          ? canonicalDigest(captured)
          : undefined,
        sourcePlan: plan ? toJson(MaintenancePlanSchema, plan) : undefined,
        proposal: result.value,
        producer: result.producerMetadata,
        execution: result.execution,
        review: {
          status: "human_review_required",
          checks:
            role === "episode_segmentation"
              ? [
                  "human boundary annotations",
                  "over/under segmentation",
                  "boundary displacement",
                ]
              : role === "journal_synthesis"
                ? [
                    "support per point",
                    "unsupported points",
                    "salient fact omissions",
                    "temporal scope",
                  ]
                : [
                    "create/revise identity",
                    "unsupported claims",
                    "contradiction handling",
                    "Schema generalization",
                    "later recall usefulness",
                  ],
        },
      },
      null,
      2,
    ) + "\n",
    { flag: "wx", mode: 0o600 },
  );
  console.log(`Proposal saved for human review: ${resolve(values.output)}`);
}

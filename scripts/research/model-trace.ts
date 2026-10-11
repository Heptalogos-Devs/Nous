// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { parseArgs } from "node:util";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { createHash } from "node:crypto";
import { canonicalDigest } from "../../apps/nous-core/src/digest.js";
import { roleNames } from "../../apps/nous-core/src/model/roles.js";
import { PromptRegistry } from "../../apps/nous-core/src/model/prompts.js";
import { providerContractForRole } from "../../apps/nous-core/src/model/schemas/contracts.js";

const { values } = parseArgs({
  options: {
    root: { type: "string" },
    attempt: { type: "string" },
    "prompt-root": { type: "string", default: "prompts" },
    "prompt-path": { type: "string" },
    "override-prompt-root": { type: "string" },
  },
});
if (!values.root || !values.attempt || !/^[1-9][0-9]*$/.test(values.attempt))
  throw new Error("Use --root <trace-root> --attempt <positive integer>");
const root = resolve(values.root, values.attempt);
const meta = JSON.parse(
  await readFile(resolve(root, "meta.json"), "utf8"),
) as Record<string, unknown> & {
  request: { file: string };
  response: { file: string };
};
async function body(
  file: string,
): Promise<Record<string, unknown> | undefined> {
  if (!/^(request|response)\.(json|txt)$/.test(file))
    throw new Error("Invalid trace body filename");
  try {
    return JSON.parse(await readFile(resolve(root, file), "utf8")) as Record<
      string,
      unknown
    >;
  } catch {
    return undefined;
  }
}
const request = await body(meta.request.file);
const response = await body(meta.response.file);
const messages = request?.messages as
  { role?: string; content?: unknown }[] | undefined;
const prompt =
  typeof request?.instructions === "string"
    ? request.instructions
    : messages
        ?.filter((message) => message.role === "system")
        .map((message) =>
          typeof message.content === "string" ? message.content : "",
        )
        .join("\n");
const promptDigest = prompt
  ? createHash("sha256").update(prompt).digest("hex")
  : undefined;
const responseFormat = request?.response_format as
  { json_schema?: { schema?: unknown } } | undefined;
const textFormat = request?.text as
  { format?: { schema?: unknown } } | undefined;
const schema =
  responseFormat?.json_schema?.schema ?? textFormat?.format?.schema;
const schemaDigest = schema ? canonicalDigest(schema) : undefined;
const prompts = new PromptRegistry(
  resolve(values["prompt-root"]!),
  values["override-prompt-root"]
    ? resolve(values["override-prompt-root"])
    : undefined,
);
const matches = [];
for (const role of roleNames) {
  const contract = providerContractForRole(role);
  const asset = await prompts.load(role, values["prompt-path"]);
  const video =
    role === "material_description"
      ? await prompts.load(role, "material/video-description.md")
      : undefined;
  const promptMatches =
    promptDigest !== undefined &&
    (asset?.digest === promptDigest || video?.digest === promptDigest);
  if (promptMatches || (schemaDigest && contract?.digest === schemaDigest))
    matches.push({
      role,
      prompt_matches: promptMatches,
      schema_matches:
        schemaDigest !== undefined && contract?.digest === schemaDigest,
    });
}
const choices = response?.choices as
  | {
      finish_reason?: string;
      message?: { content?: string; refusal?: unknown };
    }[]
  | undefined;
const output = response?.output as
  { content?: { type?: string; text?: string }[] }[] | undefined;
const outputText =
  choices?.[0]?.message?.content ??
  output
    ?.flatMap((item) => item.content ?? [])
    .filter((item) => item.type === "output_text")
    .map((item) => item.text ?? "")
    .join("");
const matchedContract = roleNames
  .map(providerContractForRole)
  .find((contract) => schemaDigest && contract?.digest === schemaDigest);
let validation;
if (matchedContract && outputText) {
  try {
    validation = matchedContract.owner.safeParse(JSON.parse(outputText)).success
      ? "accepted_by_current_zod_owner"
      : "rejected_by_current_zod_owner";
  } catch {
    validation = "not_json";
  }
}
console.log(
  JSON.stringify(
    {
      ...meta,
      model: request?.model,
      endpoint_role: (
        {
          "/embeddings": "query_embedding",
          "/rerank": "query_rerank",
          "/audio/transcriptions": "speech_transcription",
        } as Record<string, string>
      )[String(meta.endpoint)],
      prompt_digest: promptDigest,
      schema_digest: schemaDigest,
      matching_current_roles: matches,
      finish_reason: choices?.[0]?.finish_reason ?? response?.status,
      refusal: choices?.[0]?.message?.refusal,
      error: response?.error,
      current_contract_validation: validation,
      request_file: resolve(root, meta.request.file),
      response_file: resolve(root, meta.response.file),
    },
    null,
    2,
  ),
);

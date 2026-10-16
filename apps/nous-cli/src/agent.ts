// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { NousError } from "@nous-wave/client";

export class CliError extends Error {
  constructor(
    readonly code: string,
    message: string,
    readonly details: unknown[] = [],
    readonly candidates: unknown[] = [],
    readonly recovery?: string,
    readonly context: Readonly<Record<string, string>> = {},
  ) {
    super(message);
  }
}

export function uniqueReference(result: {
  status: string;
  candidates: { canonical?: { kind: string; value: string } }[];
}) {
  const canonical = result.candidates[0]?.canonical;
  if (result.status !== "BOUND" || result.candidates.length !== 1 || !canonical)
    throw new CliError(
      result.status,
      "Reference must resolve uniquely",
      [],
      result.candidates,
    );
  return canonical;
}

export function boundedInteger(
  value: string | undefined,
  min: number,
  max: number,
  name: string,
) {
  const parsed = Number(value);
  if (!Number.isInteger(parsed) || parsed < min || parsed > max)
    throw new CliError("INVALID_ARGUMENT", `${name} must be ${min}..${max}`);
  return parsed;
}

export function cliErrorPayload(error: unknown) {
  const recovery =
    error && typeof error === "object"
      ? {
          ...("receipt" in error ? { receipt: error.receipt } : {}),
          ...("notices" in error ? { notices: error.notices } : {}),
        }
      : {};
  if (error instanceof CliError)
    return {
      code: error.code,
      message: error.message,
      ...recovery,
      details: error.details,
      candidates: error.candidates,
      recovery: error.recovery,
      context: error.context,
    };
  const message =
    error instanceof Error ? error.message : "CLI operation failed";
  return {
    code: error instanceof NousError ? error.codeName : "INVALID_ARGUMENT",
    message,
    details: error instanceof NousError ? error.details : [],
    candidates: error instanceof NousError ? error.candidates : [],
    ...(error instanceof NousError
      ? { recovery: error.recovery, context: error.context }
      : {}),
    ...recovery,
  };
}

export const commandInventory = {
  nousql:
    "nous help nousql [--json]; standalone Agent guide without connecting to a daemon",
  contextFlags: {
    subject: "Subject lexical reference overrides local selection",
    session: "Session lexical reference overrides local selection",
    "work-context": "WorkContext lexical reference overrides local selection",
  },
  output: {
    success:
      "semantic text by default; --json returns nous.cli.v1 envelope; int64 as decimal strings",
    error:
      "stderr semantic error; --json provides code/message/details/candidates/receipt; nonzero exit",
  },
  commands: [
    {
      command: "clock read|configure|pause|advance|resume",
      purpose:
        "Authenticated Subject world clock; configure/advance/resume take --request-file JSON. Not a model tool.",
    },
    {
      command: "reflection status|repair",
      parameters: [
        "status [operation UUID] exposes pending/recent/blocked/keep/partial/failed and retained paid proposal",
        "repair --request-file <ReflectionRepairRequest TOML> requires explicitly reviewed exact heads and epochs; never invokes a model",
      ],
    },
    {
      command:
        "motivation list|select|adopt|get|revision|history|revise|evidence|suppress|restore|withdraw|reaccept|purge|evaluate|evaluation|evaluations",
      parameters: [
        "Typed Desire/Concern/DesiredCondition exact references; mutations and evaluate require authenticated management",
        "--request-file uses canonical MotivationContent/EvaluateConditionRequest",
      ],
    },
    {
      command:
        "social list|select|pair|neighborhood|resolve|adopt|get|revision|history|revise|evidence|suppress|restore|withdraw|reaccept|purge",
      parameters: [
        "[typed SocialRelation/LanguageConvention object or exact revision reference]",
        "adopt/revise --request-file <SocialContent TOML> --text <reason>",
        "select/neighborhood --request-file <SocialSelectionRequest TOML with trusted source_read and scope>",
        "pair/resolve --request-file <SocialPairRequest/ResolveConventionsRequest TOML>",
        "evidence --request-file <AddSocialEvidenceRequest TOML with role and basis>",
        "--expected-object-epoch <epoch>; --operation-id <stable retry id>",
      ],
    },
    {
      command:
        "self list|soul|interpret|interpret-seed|adopt|get|revision|history|revise|evidence|suppress|restore|withdraw|reaccept|purge",
      parameters: [
        "[typed SelfFacet/NarrativeThread object or exact revision reference]",
        "adopt/revise --request-file <SelfContent TOML> --text <reason>",
        "interpret --request-file <InterpretSelfRequest TOML>; interpret-seed uses the currently adopted Seed",
        "evidence --request-file <AddSelfEvidenceRequest TOML with role and basis>",
        "--expected-object-epoch <epoch>; --operation-id <stable retry id>",
      ],
      description:
        "Explicit Self management and source-backed model interpretation. Soul is a derived current view; ordinary chat does not authorize mutation.",
    },
    {
      command: "subject provision",
      parameters: ["--request-file <SubjectCapabilities TOML>"],
      description:
        "Append explicitly requested domain capabilities to the existing Subject without replacing identity or data.",
    },
    {
      command: "serving clean",
      parameters: ["--subject <Subject reference>"],
      description:
        "Reclaim retired/failed and orphan Serving assets without rebuilding; protect active readers, current and research-pinned generations.",
    },
    {
      command:
        "material clean|limits|artifacts|occurrences|representations|producer",
      parameters: [
        "clean reclaims unreferenced objects and staging across BlobRoot; no selected Subject required",
        "occurrences <Artifact reference>",
        "representations [Artifact or SourceRegion reference] [--kind <kind>]",
        "producer <signature identity>",
        "--page-size <n> --page-token <token> for artifacts",
      ],
    },
    {
      command:
        "memory form|get|list|revision|history|revise|accessibility|link|suppress|restore|withdraw|reaccept|purge",
      parameters: [
        "[Memory reference]",
        "--request-file <Memory content TOML> for form/revise",
        "list --status accepted|withdrawn (optional)",
        "--kind <revision intent or accessibility mode>",
        "--expected-object-epoch <observed epoch> skips mutation pre-read; omitted reads current epoch",
        "--page-size <n> --page-token <token>",
      ],
      description:
        "Source-backed cognition management; mutations freeze the current object epoch and stable operation identity.",
    },
    {
      command:
        "schema create|get|list|history|revision|evidence|revise|split|merge|suppress|restore|withdraw|reaccept|purge",
      parameters: [
        "[Schema reference]",
        "--request-file <Schema content/evidence/children or MergeCognitiveSchemasRequest TOML>",
        "--kind <revision intent>",
        "--expected-object-epoch <observed epoch> skips mutation pre-read; omitted reads current epoch",
      ],
      description: "Supported transferable pattern and evidence management.",
    },
    {
      command:
        "episode create|get|list|revision|history|revise|link|suppress|restore|withdraw|reaccept|purge",
      parameters: [
        "[Episode reference]",
        "--request-file <Episode content TOML>",
        "create --kind <segmentation track>",
        "--kind resegment|reinterpret",
        "--expected-object-epoch <observed epoch> skips mutation pre-read; omitted reads current epoch",
        "list --status accepted|withdrawn (optional)",
      ],
    },
    {
      command:
        "journal get|list|revision|history|suppress|restore|withdraw|reaccept|purge",
      parameters: [
        "[Journal reference]",
        "--expected-object-epoch <observed epoch> skips mutation pre-read; omitted reads current epoch",
        "list --status accepted|withdrawn (optional)",
        "--page-size <n> --page-token <token>",
      ],
    },
    {
      command: "resource put|get|list|remove|materialize",
      parameters: [
        "[Resource reference]",
        "--request-file <Resource descriptor or stable external reference TOML>",
        "list --status ready|degraded|unavailable (optional)",
      ],
    },
    {
      command: "projection build|managed",
      parameters: [
        "--consumer <operator-configured consumer>",
        "--max-items <n> --max-bytes <n>",
        "--request-file <query/situation_refs/known_cursor TOML> (optional)",
      ],
      description:
        "Generate actual consumer projection or synchronize managed context; requires selected Session and uses its foreground WorkContext.",
    },

    {
      command: "read",
      parameters: [
        "<obs:|art:|src:|repr:|region: lexical reference> | result:N",
        "--max-bytes 1..1048576 (default 65536)",
        "--output <new-file> saves complete exact bytes for native media inspection",
      ],
      description:
        "Read admitted source/derived content through Material Authority; show returns object metadata. Partial output is explicit. --output exports exact complete bytes without overwriting an existing file.",
    },
    { command: "status", description: "Instance status and capabilities" },
    {
      command: "identity resolve",
      parameters: ["--kind <kind> --name <name> | --lexical-ref <ref>"],
      example: "nous identity resolve --kind entity --name Alice",
    },
    {
      command: "identity bind",
      parameters: [
        "--kind <kind>",
        "--canonical <reference>",
        "--name <name>",
        "--alias <alias> (comma-separated)",
      ],
      example:
        "nous identity bind --kind entity --canonical entity:alice --name Alice --alias A",
    },
    {
      command: "tag list|get|search|resolve|create|revise|merge|split|attach",
      parameters: [
        "list --status active|withdrawn (defaults to active)",
        "create --name <label> [--description <text> --kind-hint <hint>] | --request-file <TagContent TOML>",
        "get <tag-ref>",
        "search <text>",
        "resolve <name>",
        "create --name <label> [--description <text>]",
        "revise|merge|split --request-file <formal Request TOML; targets include expected_revision_id>",
        "attach <exact-revision> --tag <ref> --request-file <Association TOML>",
      ],
      example: "nous tag resolve deploy",
    },
    {
      command: "association get|list|neighborhood",
      parameters: [
        "<canonical-or-lexical-ref>",
        "--kind <kind> (for lexical)",
        "--max-nodes 1..256",
        "--max-depth 1..4",
      ],
      example: "nous association neighborhood <Memory lexical reference>",
    },
    {
      command: "association create",
      parameters: [
        "--operation-id <stable operation identity> (optional; generated)",
        "--request-file <CreateAssociationRequest TOML>",
      ],
      description:
        "Formal Association request with nonempty revision/use-event basis",
    },
    {
      command: "association revoke",
      parameters: [
        "<association reference>",
        "--operation-id <stable operation identity> (optional; generated)",
      ],
      description:
        "Revoke one exact AssociationEvidence, including a Tag attachment",
    },
    {
      command: "maintenance grant",
      parameters: [
        "--max-operations 1..32",
        "--max-model-calls 0..32",
        "--max-elapsed-ms 1..900000",
      ],
      description: "Host authorizes a bounded maintenance opportunity",
    },
    {
      command: "use",
      parameters: [
        "<exact cognition revision>",
        "--kind presented|referenced|acted_on|result_supported|result_refuted|corrected|pinned",
        "--event-id <stable event identity> (optional; generated)",
        "--consumer <ref>",
        "--occurred-at <ISO timestamp>",
        "--query-id query:last (optional; result:N links automatically)",
      ],
      description:
        "Typed meaningful use; stable retry reuses event ID and occurrence timestamp",
    },
    {
      command: "query",
      parameters: ["<NousQL> | --query-file <path>"],
      example: "nous query 'deployment decision $return(memory) $limit(5)'",
    },
    {
      command: "query prepare|inspect",
      parameters: ["<NousQL> | --query-file <path>"],
      description:
        "Frozen binding and representation inspection; no retrieval or provider call",
      example: "nous query prepare 'deployment decision $return(memory)'",
    },
    {
      command: "config list|describe|get|set|clear",
      parameters: [
        "[path] [JSON value]",
        "--subject <Subject lexical reference>",
        "--desired",
        "--expected-revision <revision from desired view> (required for set/clear)",
        "--advanced|--developer",
      ],
    },
    {
      command: "subject create|list|use|seed|adopt-seed",
      parameters: [
        "[id]",
        "--operation-id <stable operation identity> (optional; generated)",
        "list --status <Subject status> (optional)",
      ],
    },
    {
      command: "session open|list|close",
      purpose: "Open a new Session or close the selected Session",
      parameters: ["list --status open|closed (optional)"],
    },
    {
      command: "session show [reference]",
      purpose:
        "Read a returned Session reference/name, including a closed Session, without selecting or reopening it; omit the reference to read local selection",
    },
    {
      command: "observe text|file",
      parameters: [
        "--text <text> --source <url> | <path>",
        "--media-type <mime>",
      ],
    },
    {
      command: "form",
      parameters: [
        "<Occurrence reference>",
        "--operation-id <stable operation identity> (optional; generated)",
        "--aboutness <ref> (comma-separated)",
        "--aboutness-mode explicit|select_from_resolved_mentions|none (defaults to explicit with --aboutness)",
        "--tag <Tag ID/ref/LexicalRef> (comma-separated)",
      ],
    },
    {
      command: "derive",
      parameters: [
        "<SourceRegion reference>",
        "--strategy description_only|direct_structured|describe_then_structure",
      ],
      description:
        "Select the derived output pipeline. Inspect video.input_mode (direct|frames) with config describe video.input_mode / config get video.input_mode. Frame sampling records source/timestamp coverage in the interpretation, without providing a separate frame Artifact export.",
    },
    { command: "embeddings prepare", parameters: ["--max-batches <n>"] },
    { command: "trace", parameters: ["<canonical-or-lexical-ref>"] },
    {
      command: "show",
      parameters: ["<exact-reference|LexicalRef|result:N>"],
      description:
        "Read exact cognition content or Material object metadata; read retrieves source text.",
    },
    {
      command: "retry",
      parameters: ["<saved-request-id> | forget <saved-request-id>"],
      description:
        "Replay original frozen inputs through the semantic owner; forget discards only local retry inputs.",
    },
    {
      command:
        "context create|set|pin|unpin|clear|select|foreground|show|pause|resume|list|end",
      parameters: [
        "[WorkContext lexical reference]",
        "--purpose <purpose>",
        "--text <task context> | --file <path|->",
        "--request-file <WorkContext fields TOML>",
        "--expected-revision <revision> (optional; otherwise reads current revision)",
        "--cognition <exact-ref|result:N> --entity <real-ref> --tag <real-ref>",
        "clear --scope text|anchors|all",
        "foreground --clear",
      ],
      example:
        'nous context create --purpose "Investigate CPython free-threading" --text "Compare PEP 703 and Python 3.13/3.14 extension compatibility."',
    },
  ],
};

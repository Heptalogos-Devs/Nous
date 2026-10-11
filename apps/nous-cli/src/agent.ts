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
  const transportCodes: Record<number, string> = {
    3: "INVALID_ARGUMENT",
    5: "NOT_FOUND",
    8: "RESOURCE_EXHAUSTED",
    9: "FAILED_PRECONDITION",
    10: "ABORTED",
    14: "UNAVAILABLE",
    4: "DEADLINE_EXCEEDED",
  };
  return {
    code:
      error instanceof NousError
        ? (error.domainCode ?? transportCodes[error.code] ?? "RPC_ERROR")
        : "INVALID_ARGUMENT",
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
        "get <tag-ref>",
        "search <text>",
        "resolve <name>",
        "create --name <label> [--description <text>]",
        "revise|merge|split --request-file <semantic TOML>",
        "attach <exact-revision> --tag <ref> --association-file <basis TOML>",
      ],
      example: "nous tag resolve deploy",
    },
    {
      command: "association neighborhood",
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
        "--association-file <semantic TOML>",
      ],
      description:
        "CLI-owned semantic Association TOML; requires nonempty revision/use-event basis",
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
    { command: "subject create|list|use", parameters: ["[id]"] },
    {
      command: "session open|close",
      purpose: "Open a new Session or close the selected Session",
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
      parameters: ["<saved-receipt-id>"],
      description:
        "Replay original frozen operation inputs; preserve the consumer state root.",
    },
    {
      command:
        "context create|set|pin|unpin|clear|select|foreground|show|pause|resume|list|end",
      parameters: [
        "[WorkContext lexical reference]",
        "--purpose <purpose>",
        "--text <task context> | --file <path|->",
        "--cognition <exact-ref|result:N> --entity <real-ref> --tag <real-ref>",
        "clear --scope text|anchors|all",
        "foreground --clear",
      ],
      example:
        'nous context create --purpose "Investigate CPython free-threading" --text "Compare PEP 703 and Python 3.13/3.14 extension compatibility."',
    },
  ],
};

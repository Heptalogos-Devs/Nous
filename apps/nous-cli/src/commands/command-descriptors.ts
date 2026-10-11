// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

type CommandFlag = {
  name: string;
  type: "string" | "boolean";
  description?: string;
};

export type CommandDescriptor = {
  command: string;
  flags?: readonly CommandFlag[];
  parameters?: readonly string[];
  description?: string;
  example?: string;
};

const string = (...names: string[]): CommandFlag[] =>
  names.map((name) => ({ name, type: "string" }));
const boolean = (...names: string[]): CommandFlag[] =>
  names.map((name) => ({ name, type: "boolean" }));
const command = (
  name: string,
  flags: CommandFlag[] = [],
  parameters: string[] = [],
  description?: string,
  example?: string,
): CommandDescriptor => ({
  command: name,
  flags,
  parameters,
  description,
  example,
});

/** One command-owned inventory drives citty parsing and all native help surfaces. */
export const commandDescriptors: readonly CommandDescriptor[] = [
  command(
    "help",
    [],
    ["[topic]"],
    "Show command inventory without connecting to Core.",
  ),
  command("status", [], [], "Instance status and capabilities."),
  command(
    "config list",
    [...boolean("advanced")],
    [],
    "List descriptors at the selected exposure level.",
  ),
  command(
    "config describe",
    [],
    ["<path>"],
    "Show one descriptor, schema, scope and effect.",
  ),
  command(
    "config get",
    [...string("view")],
    ["<path>", "--view active|desired|both (default both)"],
    "Read active and/or desired configuration projections.",
  ),
  command(
    "config set",
    [...string("value-json", "expected-revision", "operation-id")],
    [
      "<path>",
      "--value-json <complete JSON value>",
      "--expected-revision <desired revision>",
    ],
    "Set a complete JSON value and report active/desired state.",
  ),
  command(
    "config clear",
    [...string("expected-revision", "operation-id")],
    ["<path>", "--expected-revision <desired revision>"],
    "Clear an override and report active/desired state.",
  ),
  command(
    "subject create",
    [...string("request-file")],
    ["--request-file <semantic TOML> (required)"],
    "Create a Subject with a stable ID and an explicit valid Cognitive Seed frozen before the operation receipt; metadata and capabilities are optional.",
  ),
  command("subject list"),
  command("subject get", [], ["<Subject reference>"]),
  command("subject seed", [], ["<Subject reference>"]),
  command(
    "subject adopt-seed",
    [...string("request-file", "operation-id")],
    ["<Subject reference>"],
    "Adopt a TOML Cognitive Seed using an explicit initial or import kind.",
  ),
  command("subject use", [], ["<Subject reference>"]),
  command("session open"),
  command(
    "session show",
    [],
    ["[Session reference]"],
    "Read a Session without opening or selecting it.",
  ),
  command("session close"),
  command(
    "observe text",
    [...string("text", "source")],
    ["--text <text>", "--source <https URL> (optional)"],
    "Admit bounded inline text as an Observation.",
  ),
  command(
    "observe file",
    [...string("media-type")],
    ["<path>", "--media-type <mime>"],
    "Upload an Artifact and admit it as an Observation.",
  ),
  command(
    "derive",
    [...string("strategy", "target", "supersedes")],
    ["<SourceRegion reference>", "--strategy <strategy>"],
    "Derive material with an explicit strategy.",
  ),
  command(
    "form",
    [
      ...string("operation-id", "aboutness-mode", "representation"),
      ...string("aboutness", "tag"),
    ],
    ["<Occurrence reference>"],
    "Form cognition from an admitted Observation.",
  ),
  command("embeddings prepare", [...string("max-batches")]),
  command("identity bind", [...string("kind", "canonical", "name", "alias")]),
  command("identity resolve", [...string("kind", "name", "lexical-ref")]),
  command(
    "identity rebind",
    [...string("binding-state", "entity-ref", "host-resolution-ref", "reason")],
    ["<Mention UUID>"],
    "Revise an entity mention binding with an explicit state.",
  ),
  command("maintenance grant", [
    ...string("max-operations", "max-model-calls", "max-elapsed-ms"),
  ]),
  command("tag create", [
    ...string("name", "description", "kind-hint", "operation-id"),
  ]),
  command("tag get", [], ["<Tag reference>"]),
  command("tag list", [...string("page-size", "page-token")]),
  command("tag search", [...string("page-size", "page-token")], ["<text>"]),
  command("tag resolve", [], ["<name>"]),
  command("tag revise", [...string("request-file", "operation-id")]),
  command("tag merge", [...string("request-file", "operation-id")]),
  command("tag split", [...string("request-file", "operation-id")]),
  command(
    "tag attach",
    [...string("association-file", "operation-id", "tag")],
    ["<exact cognition revision>"],
  ),
  command("association create", [
    ...string("association-file", "operation-id"),
  ]),
  command(
    "association revoke",
    [...string("operation-id")],
    ["<Association reference>"],
  ),
  command(
    "association neighborhood",
    [...string("kind", "max-nodes", "max-depth")],
    ["<reference>"],
  ),
  command(
    "memory list",
    [...string("page-size", "page-token", "status")],
    [],
    "List paged Memories for the selected Subject.",
  ),
  command("memory get", [], ["<Memory reference>"]),
  command("memory revision", [], ["<exact Memory revision reference>"]),
  command(
    "memory history",
    [...string("page-size", "page-token")],
    ["<Memory reference>"],
  ),
  command(
    "memory revise",
    [...string("request-file", "expected-epoch", "operation-id")],
    ["<Memory reference>"],
    "Revise Memory using semantic TOML and the expected object epoch.",
  ),
  command(
    "memory suppress",
    [...string("expected-epoch", "operation-id")],
    ["<Memory reference>"],
  ),
  command(
    "memory restore",
    [...string("expected-epoch", "operation-id")],
    ["<Memory reference>"],
  ),
  command(
    "memory withdraw",
    [...string("expected-epoch", "operation-id")],
    ["<Memory reference>"],
  ),
  command(
    "memory reaccept",
    [...string("expected-epoch", "operation-id")],
    ["<Memory reference>"],
  ),
  command(
    "memory purge",
    [...string("expected-epoch", "operation-id")],
    ["<Memory reference>"],
  ),
  command(
    "memory accessibility",
    [...string("mode", "expected-epoch", "operation-id")],
    ["<Memory reference>"],
  ),
  command("memory link", [...string("from", "to", "relation", "operation-id")]),
  command("resource list", [...string("page-size", "page-token", "status")]),
  command("resource put", [...string("request-file")]),
  command("resource get", [], ["<Resource reference>"]),
  command("resource remove", [], ["<Resource reference>"]),
  command("resource materialize", [...string("request-file", "operation-id")]),
  command("material limits"),
  command(
    "material producer",
    [],
    ["<producer_signature_id UUID>"],
    "Read a producer by the canonical ID returned on a derived representation; the signature hash is not an ID.",
  ),
  command("material artifacts", [...string("page-size", "page-token")]),
  command("material representations", [
    ...string("source-region", "artifact", "kind", "limit"),
  ]),
  command("schema create", [...string("request-file", "operation-id")]),
  command("schema get", [], ["<CognitiveSchema reference>"]),
  command(
    "schema revision",
    [],
    ["<exact CognitiveSchema revision reference>"],
  ),
  command(
    "schema evidence",
    [...string("request-file", "expected-epoch", "operation-id")],
    ["<CognitiveSchema reference>"],
  ),
  command(
    "schema revise",
    [...string("request-file", "expected-epoch", "operation-id")],
    ["<CognitiveSchema reference>"],
  ),
  command(
    "schema split",
    [...string("request-file", "expected-epoch", "operation-id")],
    ["<CognitiveSchema reference>"],
  ),
  command("schema merge", [...string("request-file", "operation-id")]),
  command(
    "schema suppress",
    [...string("expected-epoch", "operation-id")],
    ["<CognitiveSchema reference>"],
  ),
  command(
    "schema restore",
    [...string("expected-epoch", "operation-id")],
    ["<CognitiveSchema reference>"],
  ),
  command(
    "schema withdraw",
    [...string("expected-epoch", "operation-id")],
    ["<CognitiveSchema reference>"],
  ),
  command(
    "schema reaccept",
    [...string("expected-epoch", "operation-id")],
    ["<CognitiveSchema reference>"],
  ),
  command(
    "schema purge",
    [...string("expected-epoch", "operation-id")],
    ["<CognitiveSchema reference>"],
  ),
  command("episode list", [...string("page-size", "page-token", "status")]),
  command("episode get", [], ["<Episode reference>"]),
  command("episode revision", [], ["<exact Episode revision reference>"]),
  command(
    "episode history",
    [...string("page-size", "page-token")],
    ["<Episode reference>"],
  ),
  command("episode create", [...string("request-file", "operation-id")]),
  command(
    "episode revise",
    [...string("request-file", "expected-epoch", "intent", "operation-id")],
    ["<Episode reference>"],
  ),
  command("episode link", [
    ...string("from", "to", "relation", "operation-id"),
  ]),
  command(
    "episode suppress",
    [...string("expected-epoch", "operation-id")],
    ["<Episode reference>"],
  ),
  command(
    "episode restore",
    [...string("expected-epoch", "operation-id")],
    ["<Episode reference>"],
  ),
  command(
    "episode withdraw",
    [...string("expected-epoch", "operation-id")],
    ["<Episode reference>"],
  ),
  command(
    "episode reaccept",
    [...string("expected-epoch", "operation-id")],
    ["<Episode reference>"],
  ),
  command(
    "episode purge",
    [...string("expected-epoch", "operation-id")],
    ["<Episode reference>"],
  ),
  command("journal list", [...string("page-size", "page-token", "status")]),
  command("journal get", [], ["<Journal reference>"]),
  command("journal revision", [], ["<exact Journal revision reference>"]),
  command(
    "journal history",
    [...string("page-size", "page-token")],
    ["<Journal reference>"],
  ),
  command(
    "journal suppress",
    [...string("expected-epoch", "operation-id")],
    ["<Journal reference>"],
  ),
  command(
    "journal restore",
    [...string("expected-epoch", "operation-id")],
    ["<Journal reference>"],
  ),
  command(
    "journal withdraw",
    [...string("expected-epoch", "operation-id")],
    ["<Journal reference>"],
  ),
  command(
    "journal reaccept",
    [...string("expected-epoch", "operation-id")],
    ["<Journal reference>"],
  ),
  command(
    "journal purge",
    [...string("expected-epoch", "operation-id")],
    ["<Journal reference>"],
  ),
  command(
    "query",
    [...string("query-file")],
    ["<NousQL> | --query-file <path>"],
    "Execute NousQL and save exact result references.",
    "nous query 'deployment decision $return(memory) $limit(5)'",
  ),
  command(
    "query prepare",
    [...string("query-file")],
    ["<NousQL> | --query-file <path>"],
    "Prepare and inspect a frozen query without retrieval or provider calls.",
  ),
  command(
    "query inspect",
    [...string("query-file")],
    ["<NousQL> | --query-file <path>"],
    "Inspect a frozen query without retrieval or provider calls.",
  ),
  command(
    "use",
    [...string("kind", "event-id", "occurred-at", "query-id")],
    ["<exact cognition revision>"],
  ),
  command("trace", [], ["<canonical or lexical reference>"]),
  command("show", [], ["<exact reference | result:N>"]),
  command(
    "read",
    [...string("max-bytes", "output")],
    ["<Material reference | result:N>"],
  ),
  command("context create", [
    ...string("purpose", "text", "file", "operation-id"),
  ]),
  command("context list"),
  command("context show", [], ["[WorkContext reference]"]),
  command(
    "context set",
    [...string("purpose", "text", "file", "operation-id")],
    ["[WorkContext reference]"],
  ),
  command("context pin", [
    ...string("cognition", "entity", "tag", "operation-id"),
  ]),
  command("context unpin", [
    ...string("cognition", "entity", "tag", "operation-id"),
  ]),
  command(
    "context clear",
    [...string("scope", "operation-id")],
    [],
    "Clear text, anchors, or both with --scope text|anchors|all.",
  ),
  command("context pause", [...string("operation-id")]),
  command("context resume", [...string("operation-id")]),
  command("context end", [...string("operation-id")]),
  command("context select", [], ["[WorkContext reference]"]),
  command(
    "context foreground",
    [...boolean("clear"), ...string("operation-id")],
    ["[WorkContext reference]"],
  ),
  command(
    "retry",
    [],
    ["<saved receipt ID>"],
    "Replay the original frozen request with its original identity.",
  ),
  command(
    "operation inspect",
    [],
    [
      "<saved receipt ID> | mutation <operation-kind> <subject-id> <operation-id> | subject-create <subject-id> <operation-id> | configuration <operation-id> | observation <subject-id> <request-id> | workflow <subject-id> <memory|material> <operation-key>",
    ],
    "Inspect a saved receipt or query owner-backed status using an explicit typed identity; never displays request or result bodies.",
    "nous operation inspect mutation revise_memory <subject-id> <operation-id>",
  ),
];

const commonFlags: CommandFlag[] = [
  ...string(
    "run-root",
    "instance-root",
    "consumer",
    "subject",
    "session",
    "work-context",
  ),
  ...boolean("json", "raw", "developer"),
];

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
      "nous.cli.v2 envelope; complete command data; protobuf 64-bit integers use decimal strings",
    error:
      "stderr safe error envelope with owner/code/recovery/context/trace and receipt when available; nonzero exit",
  },
  commands: commandDescriptors,
};

export const commandFlags = (descriptor: CommandDescriptor) => [
  ...commonFlags,
  ...(descriptor.flags ?? []),
];

export const sharedFlags = commonFlags;

// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import { create, fromJson, toJson } from "@bufbuild/protobuf";
import { TimestampSchema, type Timestamp } from "@bufbuild/protobuf/wkt";
import { Code, ConnectError } from "@connectrpc/connect";
import {
  QueryExprSchema,
  QueryModifiersSchema,
  QueryConstraintsSchema,
  CueSchema,
  type QueryExpr,
  type QueryModifiers,
  type Cue,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import type { Ref } from "../domain.js";
import {
  canonical,
  directiveText,
  parse,
  resultDomainOrder,
} from "./parser.js";
import type { Atom, Directive, Locator } from "./syntax.js";

export type IdentityResolver = (
  kind: string,
  locator: Locator,
  asOf?: Timestamp,
) => Promise<{ canonical: Ref; lexicalRef: string }>;
const selectorKinds = {
  e: "entity",
  tag: "tag",
  schema: "cognitive_schema",
  r: "resource",
  ref: "",
  object: "external_object",
};
function invalid(message: string): never {
  throw new ConnectError(message, Code.InvalidArgument);
}

export async function compileNousQL(
  source: string,
  resolve: IdentityResolver,
  now: Timestamp,
) {
  const syntax = parse(source);
  const sourceCanonical = canonical(syntax);
  const temporalExpressions = syntax.directives
    .filter((d) => ["time", "asof", "history"].includes(d.name))
    .map(directiveText);
  let authorityTime: Timestamp | undefined;
  async function bindAtom(atom: Atom): Promise<Cue[]> {
    if (atom.kind === "text" || atom.kind === "concept")
      return [
        create(CueSchema, {
          cue: {
            case: atom.kind === "text" ? "text" : "concept",
            value: atom.text,
          },
        }),
      ];
    const bound = await Promise.all(
      atom.locators.map(async (locator) => {
        if (atom.selector === "object")
          return {
            canonical: { kind: "external_object", value: locator.value },
            lexicalRef: "",
          };
        return resolve(selectorKinds[atom.selector], locator, authorityTime);
      }),
    );
    const identities = bound.map(
      (b) => `${b.canonical.kind}:${b.canonical.value}`,
    );
    if (new Set(identities).size !== identities.length)
      invalid("DUPLICATE_ENTITY_PARTICIPANT");
    if (atom.selector === "e")
      bound.sort((a, b) => a.lexicalRef.localeCompare(b.lexicalRef));
    if (atom.selector !== "object")
      atom.locators = bound.map((b) => ({
        kind: "lexical",
        value: b.lexicalRef,
      }));
    return bound.map((b) => {
      const cases = {
        e: "entityRef",
        tag: "tagId",
        schema: "schemaId",
        r: "resourceRef",
        object: "externalObjectRef",
      } as const;
      return create(CueSchema, {
        cue:
          atom.selector === "ref"
            ? { case: "reference", value: b.canonical }
            : { case: cases[atom.selector], value: b.canonical.value },
      });
    });
  }
  async function lower(): Promise<QueryExpr> {
    const modifiers = create(QueryModifiersSchema, { constraints: {} });
    for (const directive of syntax.directives)
      applyDirective(modifiers, directive, now);
    authorityTime = modifiers.asOf ?? (modifiers.history ? now : undefined);
    for (const preference of syntax.preferences) {
      if (preference.operand.kind === "key") {
        if (
          !/^recent:(occurred|observed|valid|formed|recorded)$/.test(
            preference.operand.value,
          )
        )
          invalid("Unknown soft preference key");
        modifiers.preferences.push({
          $typeName: "nous.wave.v1alpha1.Preference",
          negative: preference.negative,
          key: preference.operand.value,
        });
      } else {
        const cues = await bindAtom(preference.operand);
        for (const cue of cues)
          modifiers.preferences.push({
            $typeName: "nous.wave.v1alpha1.Preference",
            negative: preference.negative,
            key: "",
            cue,
          });
      }
    }
    return create(QueryExprSchema, {
      operation: "atom",
      cues: [
        ...(await bindAtom({ kind: "text", text: syntax.intentText })),
        ...(await Promise.all(syntax.cues.map(bindAtom))).flat(),
      ],
      children: [],
      modifiers,
    });
  }
  const expression = await lower();
  expression.modifiers ??= create(QueryModifiersSchema);
  expression.modifiers.clockNow = now;
  expression.modifiers.temporalExpressions = temporalExpressions;
  return { expression, sourceCanonical, boundCanonical: canonical(syntax) };
}
function applyDirective(m: QueryModifiers, d: Directive, now: Timestamp) {
  const c = (m.constraints ??= create(QueryConstraintsSchema));
  const strings = () => {
    if (
      Object.keys(d.named).length ||
      !d.positional.length ||
      d.positional.some((v) => typeof v !== "string")
    )
      invalid(`Invalid arguments for $${d.name}`);
    const values: string[] = [];
    for (const value of d.positional) {
      if (typeof value !== "string")
        invalid(`Invalid arguments for $${d.name}`);
      values.push(value);
    }
    return values;
  };
  const one = () => {
    const v = strings();
    if (v.length !== 1) invalid(`$${d.name} expects one argument`);
    return v[0]!;
  };
  const noArguments = () => {
    if (d.positional.length || Object.keys(d.named).length)
      invalid(`$${d.name} takes no arguments`);
  };
  switch (d.name) {
    case "return": {
      const domains = strings();
      if (
        new Set(domains).size !== domains.length ||
        domains.some((v) => !["cognition", ...resultDomainOrder].includes(v)) ||
        (domains.includes("cognition") && domains.length !== 1)
      )
        invalid("Invalid result projection");
      const selected =
        domains[0] === "cognition" ? resultDomainOrder.slice(0, 4) : domains;
      m.projection = {
        $typeName: "nous.wave.v1alpha1.ResultProjection",
        domains: [...selected].sort(
          (a, b) => resultDomainOrder.indexOf(a) - resultDomainOrder.indexOf(b),
        ),
      };
      d.positional =
        domains[0] === "cognition" ? ["cognition"] : m.projection.domains;
      break;
    }
    case "asof": {
      let point: Timestamp;
      if (d.positional.length === 1 && !Object.keys(d.named).length)
        point = timestamp(d.positional[0]!);
      else if (
        !d.positional.length &&
        Object.keys(d.named).length === 1 &&
        d.named.ago !== undefined
      )
        point = subtractDuration(now, d.named.ago);
      else invalid("Use $asof(timestamp) or $asof(ago=duration)");
      m.asOf = point;
      d.positional = [timestampText(point)];
      d.named = {};
      break;
    }
    case "history":
      noArguments();
      m.history = true;
      break;
    case "persona":
    case "relation":
    case "rel":
      throw new ConnectError(
        `${d.name} cognition is unavailable`,
        Code.FailedPrecondition,
      );
    case "effort": {
      const value = one();
      if (!["light", "normal", "deep", "maximum"].includes(value))
        invalid("Invalid effort");
      m.effort = value;
      break;
    }
    case "limit": {
      const value = d.positional[0];
      if (
        d.positional.length !== 1 ||
        Object.keys(d.named).length ||
        typeof value !== "number" ||
        !Number.isInteger(value) ||
        value < 1 ||
        value > 2048
      )
        invalid("Invalid limit");
      m.limit = value;
      break;
    }
    case "source":
      c.sourceClassesInclude = strings();
      break;
    case "modality":
      c.modalities = strings();
      break;
    case "cognitiveRole":
      c.cognitiveRoles = strings();
      break;
    case "formationMode":
      c.formationModes = strings();
      break;
    case "evidenceClass":
      c.evidenceClasses = strings();
      break;
    case "authority":
      c.authority = one();
      break;
    case "current": {
      const value = one();
      if (!["none", "prefer", "required"].includes(value))
        invalid("Invalid current-authority requirement");
      m.currentAuthority = value;
      break;
    }
    case "diagnostics": {
      const value = one();
      if (!["none", "summary", "full"].includes(value))
        invalid("Invalid diagnostics level");
      m.diagnostics = value;
      break;
    }
    case "explore":
      noArguments();
      m.exploration = "bounded_associative";
      break;
    case "materialize":
      noArguments();
      m.materialize = true;
      break;
    case "exclude": {
      if (
        d.positional.length ||
        Object.keys(d.named).length !== 1 ||
        typeof d.named.source !== "string"
      )
        invalid("Use $exclude(source=...)");
      c.sourceClassesExclude.push(d.named.source);
      break;
    }
    case "time": {
      const axis = d.positional[0];
      if (
        d.positional.length !== 1 ||
        !["occurred", "observed", "valid", "formed", "recorded"].includes(
          String(axis),
        )
      )
        invalid(
          "Time axis must be occurred, observed, valid, formed or recorded",
        );
      const keys = Object.keys(d.named);
      if (
        !keys.length ||
        keys.some((k) => !["within", "from", "to", "at"].includes(k))
      )
        invalid("Invalid time arguments");
      const key = axis as
        "occurred" | "observed" | "valid" | "formed" | "recorded";
      if (d.named.at !== undefined) {
        if (keys.length !== 1)
          invalid("at cannot be combined with other time arguments");
        const point = timestamp(d.named.at);
        c[key] = {
          $typeName: "nous.wave.v1alpha1.TimePredicate",
          predicate: { case: "point", value: point },
        };
        d.named = { at: timestampText(point) };
        break;
      }
      let start: Timestamp | undefined;
      let end: Timestamp | undefined;
      if (d.named.within !== undefined) {
        if (keys.length !== 1)
          invalid("within cannot be combined with other time arguments");
        end = now;
        start = subtractDuration(now, d.named.within);
      } else {
        start =
          d.named.from === undefined ? undefined : timestamp(d.named.from);
        end = d.named.to === undefined ? undefined : timestamp(d.named.to);
      }
      if (
        start &&
        end &&
        (start.seconds > end.seconds ||
          (start.seconds === end.seconds && start.nanos >= end.nanos))
      )
        invalid("Time range requires start < end");
      c[key] = {
        $typeName: "nous.wave.v1alpha1.TimePredicate",
        predicate: {
          case: "range",
          value: { $typeName: "nous.wave.v1alpha1.TimeInterval", start, end },
        },
      };
      d.named = {
        ...(start ? { from: timestampText(start) } : {}),
        ...(end ? { to: timestampText(end) } : {}),
      };
      break;
    }
    default:
      invalid(`Unknown directive $${d.name}`);
  }
}
function timestamp(value: string | number): Timestamp {
  if (
    typeof value !== "string" ||
    !/^\d{4}-\d\d-\d\dT.*(?:Z|[+-]\d\d:\d\d)$/.test(value)
  )
    invalid("Time requires an explicit ISO-8601 offset");
  try {
    return fromJson(TimestampSchema, value);
  } catch {
    return invalid("Invalid timestamp");
  }
}
function timestampText(value: Timestamp) {
  return toJson(TimestampSchema, value) as string;
}
function subtractDuration(value: Timestamp, duration: string | number) {
  const nanos =
    value.seconds * 1_000_000_000n +
    BigInt(value.nanos) -
    BigInt(durationMillis(duration)) * 1_000_000n;
  const remainder =
    ((nanos % 1_000_000_000n) + 1_000_000_000n) % 1_000_000_000n;
  const result = create(TimestampSchema, {
    seconds: (nanos - remainder) / 1_000_000_000n,
    nanos: Number(remainder),
  });
  timestampText(result); // Apply canonical Timestamp bounds to relative results too.
  return result;
}

function durationMillis(value: string | number) {
  const match = /^(\d+)(ms|s|m|h|d|w)$/.exec(String(value));
  if (!match) invalid("Invalid duration");
  const scale: Record<string, number> = {
    ms: 1,
    s: 1000,
    m: 60_000,
    h: 3_600_000,
    d: 86_400_000,
    w: 604_800_000,
  };
  const duration = Number(match[1]) * scale[match[2]!]!;
  if (!Number.isSafeInteger(duration) || duration <= 0)
    invalid("Invalid duration");
  return duration;
}

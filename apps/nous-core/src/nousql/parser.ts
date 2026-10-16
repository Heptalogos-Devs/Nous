// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0

import {
  createToken,
  EmbeddedActionsParser,
  Lexer,
  type CustomPatternMatcherFunc,
} from "chevrotain";
import { Code, ConnectError } from "@connectrpc/connect";
import type {
  Atom,
  Argument,
  Directive,
  QuerySyntax,
  Selector,
} from "./syntax.js";

function invalid(message: string): never {
  throw new ConnectError(message, Code.InvalidArgument);
}
function boundary(text: string, offset: number) {
  return offset === 0 || !/[\p{L}\p{N}_\\]/u.test(text[offset - 1]!);
}
function pattern(
  regex: RegExp,
  needsBoundary = true,
): CustomPatternMatcherFunc {
  return (text, offset) =>
    needsBoundary && !boundary(text, offset)
      ? null
      : regex.exec(text.slice(offset));
}
const selectorPattern = /^@(e|tag|schema|r|object|ref)\(/;
const directivePattern = /^\$[A-Za-z_][A-Za-z0-9_]*/;
const conceptPattern = /^#(?:\(|[\p{L}\p{N}_][\p{L}\p{N}_-]*)/u;
function startsIsland(text: string, offset: number) {
  const rest = text.slice(offset);
  return (
    /^\\[$@#\\]/.test(rest) ||
    (boundary(text, offset) &&
      (selectorPattern.test(rest) ||
        directivePattern.test(rest) ||
        conceptPattern.test(rest)))
  );
}
const Escape = createToken({ name: "Escape", pattern: /\\[$@#\\]/ });
const SelectorStart = createToken({
  name: "SelectorStart",
  pattern: pattern(selectorPattern),
  push_mode: "island",
  line_breaks: false,
});
const DirectiveStart = createToken({
  name: "DirectiveStart",
  pattern: pattern(/^\$[A-Za-z_][A-Za-z0-9_]*\(/),
  push_mode: "island",
  line_breaks: false,
});
const DirectiveFlag = createToken({
  name: "DirectiveFlag",
  pattern: pattern(directivePattern),
  line_breaks: false,
});
const ConceptStart = createToken({
  name: "ConceptStart",
  pattern: pattern(/^#\(/),
  push_mode: "island",
  line_breaks: false,
});
const Concept = createToken({
  name: "Concept",
  pattern: pattern(/^#[\p{L}\p{N}_][\p{L}\p{N}_-]*/u),
  line_breaks: false,
});
const Text = createToken({
  name: "Text",
  line_breaks: true,
  pattern: ((text, offset) => {
    let end = offset;
    while (end < text.length && !startsIsland(text, end)) end++;
    return end === offset ? null : [text.slice(offset, end)];
  }) as CustomPatternMatcherFunc,
});
const Space = createToken({
  name: "Space",
  pattern: /\s+/,
  group: Lexer.SKIPPED,
});
const Quoted = createToken({
  name: "Quoted",
  pattern: /"(?:[^"\\\r\n]|\\.)*"/,
});
const lexicalPattern =
  /[a-z]+:(?:[bdfghjklmnprstvz][aiou][bdfghjklmnprstvz][aiou][bdfghjklmnprstvz]-){2}[bdfghjklmnprstvz][aiou][bdfghjklmnprstvz][aiou][bdfghjklmnprstvz]\b/;
const lexicalReference = new RegExp(`^(?:${lexicalPattern.source})$`);
const Lexical = createToken({
  name: "Lexical",
  pattern: lexicalPattern,
});
const Duration = createToken({
  name: "Duration",
  pattern: /\d+(?:ms|s|m|h|d|w)\b/,
});
const NumberToken = createToken({ name: "Number", pattern: /\d+(?:\.\d+)?/ });
const Name = createToken({ name: "Name", pattern: /[A-Za-z_][A-Za-z0-9_]*/ });
const Comma = createToken({ name: "Comma", pattern: /,/ });
const Equals = createToken({ name: "Equals", pattern: /=/ });
const Close = createToken({ name: "Close", pattern: /\)/, pop_mode: true });
const textTokens = [
  Escape,
  SelectorStart,
  DirectiveStart,
  DirectiveFlag,
  ConceptStart,
  Concept,
  Text,
];
const islandTokens = [
  Space,
  Quoted,
  Lexical,
  Duration,
  NumberToken,
  Name,
  Comma,
  Equals,
  Close,
];
const lexer = new Lexer(
  { modes: { text: textTokens, island: islandTokens }, defaultMode: "text" },
  { ensureOptimizations: false },
);
function quoted(image: string): string {
  try {
    const value: unknown = JSON.parse(image);
    if (typeof value === "string") return value;
  } catch {
    /* handled below */
  }
  return invalid("Invalid quoted string escape");
}

class StreamParser extends EmbeddedActionsParser {
  constructor() {
    super([...textTokens, ...islandTokens]);
    this.performSelfAnalysis();
  }
  argument = this.RULE("argument", (): Argument =>
    this.OR([
      {
        ALT: () => {
          const token = this.CONSUME(Quoted);
          return this.ACTION(() => quoted(token.image));
        },
      },
      {
        ALT: () => {
          const token = this.CONSUME(Lexical);
          return this.ACTION(() => token.image);
        },
      },
      {
        ALT: () => {
          const token = this.CONSUME(Duration);
          return this.ACTION(() => token.image);
        },
      },
      {
        ALT: () => {
          const token = this.CONSUME(NumberToken);
          return this.ACTION(() => Number(token.image));
        },
      },
      {
        ALT: () => {
          const token = this.CONSUME(Name);
          return this.ACTION(() => token.image);
        },
      },
    ]),
  );
  arguments = this.RULE(
    "arguments",
    (): Pick<Directive, "positional" | "named"> => {
      const output: Pick<Directive, "positional" | "named"> = {
        positional: [],
        named: {},
      };
      this.MANY_SEP({
        SEP: Comma,
        DEF: () =>
          this.OR([
            {
              GATE: () =>
                this.LA(1).tokenType === Name &&
                this.LA(2).tokenType === Equals,
              ALT: () => {
                const key = this.CONSUME(Name);
                this.CONSUME(Equals);
                const value = this.SUBRULE(this.argument);
                this.ACTION(() => {
                  if (Object.hasOwn(output.named, key.image))
                    invalid("Duplicate named argument");
                  output.named[key.image] = value;
                });
              },
            },
            {
              ALT: () => {
                const value = this.SUBRULE2(this.argument);
                this.ACTION(() => {
                  if (Object.keys(output.named).length)
                    invalid("Positional argument after named argument");
                  output.positional.push(value);
                });
              },
            },
          ]),
      });
      this.CONSUME(Close);
      return output;
    },
  );
  query = this.RULE("query", (): QuerySyntax => {
    const output: QuerySyntax = {
      source: "",
      intentText: "",
      cues: [],
      directives: [],
      preferences: [],
    };
    this.MANY(() =>
      this.OR([
        {
          ALT: () => {
            const token = this.CONSUME(Text);
            this.ACTION(() => {
              output.intentText += token.image;
            });
          },
        },
        {
          ALT: () => {
            const token = this.CONSUME(Escape);
            this.ACTION(() => {
              output.intentText += token.image.slice(1);
            });
          },
        },
        {
          ALT: () => {
            const token = this.CONSUME(Concept);
            this.ACTION(() => {
              output.cues.push({ kind: "concept", text: token.image.slice(1) });
            });
          },
        },
        {
          ALT: () => {
            this.CONSUME(ConceptStart);
            const token = this.CONSUME(Quoted);
            this.CONSUME(Close);
            this.ACTION(() => {
              const value = quoted(token.image);
              if (!value.trim()) invalid("Empty concept");
              output.cues.push({ kind: "concept", text: value });
            });
          },
        },
        {
          ALT: () => {
            const start = this.CONSUME(SelectorStart);
            const firstArgument = this.LA(1);
            const args = this.SUBRULE(this.arguments);
            this.ACTION(() => {
              const selector = start.image.slice(1, -1) as Selector["selector"];
              if (
                Object.keys(args.named).length ||
                !args.positional.length ||
                args.positional.length > 32 ||
                (selector !== "e" && args.positional.length !== 1)
              )
                invalid("Invalid selector arity");
              if (selector === "object" && firstArgument.tokenType !== Quoted)
                invalid("@object requires a quoted opaque Host ref");
              const locators = args.positional.map((value) => {
                if (typeof value !== "string")
                  invalid("Selectors require names or LexicalRefs");
                return {
                  kind:
                    selector !== "object" && lexicalReference.test(value)
                      ? ("lexical" as const)
                      : ("name" as const),
                  value,
                };
              });
              if (selector === "ref" && locators[0]?.kind !== "lexical")
                invalid("@ref requires a LexicalRef");
              if (selector === "object" && locators[0]?.kind !== "name")
                invalid("@object requires an opaque Host ref");
              output.cues.push({ kind: "selector", selector, locators });
            });
          },
        },
        {
          ALT: () => {
            const start = this.CONSUME(DirectiveStart);
            const args = this.SUBRULE2(this.arguments);
            this.ACTION(() => {
              addDirective(output, { name: start.image.slice(1, -1), ...args });
            });
          },
        },
        {
          ALT: () => {
            const token = this.CONSUME(DirectiveFlag);
            this.ACTION(() => {
              addDirective(output, {
                name: token.image.slice(1),
                positional: [],
                named: {},
              });
            });
          },
        },
      ]),
    );
    return output;
  });
}
const parser = new StreamParser();
const knownDirectives = new Set([
  "return",
  "asof",
  "history",
  "effort",
  "limit",
  "source",
  "modality",
  "cognitiveRole",
  "formationMode",
  "evidenceClass",
  "authority",
  "current",
  "diagnostics",
  "explore",
  "materialize",
  "exclude",
  "time",
]);
function addDirective(output: QuerySyntax, directive: Directive) {
  if (directive.name === "prefer" || directive.name === "avoid") {
    if (Object.keys(directive.named).length)
      invalid("Preferences do not accept named arguments");
    const args = directive.positional;
    let operand: QuerySyntax["preferences"][number]["operand"];
    if (
      args.length === 2 &&
      args[0] === "recent" &&
      typeof args[1] === "string" &&
      ["occurred", "observed", "valid", "formed", "recorded"].includes(args[1])
    )
      operand = { kind: "key", value: `recent:${args[1]}` };
    else if (args.length === 1 && typeof args[0] === "string" && args[0].trim())
      operand = { kind: "text", text: args[0] };
    else invalid("Preferences require text or recent,time-axis");
    output.preferences.push({ negative: directive.name === "avoid", operand });
    return;
  }
  if (!knownDirectives.has(directive.name))
    invalid(`Unknown directive $${directive.name}`);
  const key =
    directive.name === "time"
      ? `time:${directive.positional[0]}`
      : directive.name;
  if (
    output.directives.some(
      (d) => (d.name === "time" ? `time:${d.positional[0]}` : d.name) === key,
    )
  )
    invalid(`Duplicate directive $${key}`);
  output.directives.push(directive);
}
export const resultDomainOrder = [
  "memory",
  "schema",
  "episode",
  "journal",
  "social_relation",
  "desire",
  "concern",
  "desired_condition",
  "language_convention",
  "self_facet",
  "narrative_thread",
  "evidence",
  "resource",
];
export function parse(source: string): QuerySyntax {
  if (Buffer.byteLength(source) > 32768)
    throw new ConnectError(
      "NousQL source exceeds 32 KiB",
      Code.ResourceExhausted,
    );
  if (/^\s*(?:"|\(\s*["@*]|\*\s*(?:[$@#]|$)|[+-]\s*")/.test(source))
    invalid(
      "Legacy expression roots are not supported; provide unquoted intent text",
    );
  const lexed = lexer.tokenize(source);
  if (lexed.errors.length)
    invalid(`NousQL lexical error at ${lexed.errors[0]!.offset}`);
  if (lexed.tokens.length > 2048)
    throw new ConnectError(
      "NousQL token bound exceeded",
      Code.ResourceExhausted,
    );
  parser.input = lexed.tokens;
  const output = parser.query();
  if (parser.errors.length)
    invalid(
      `Malformed NousQL syntax island at ${parser.errors[0]!.token.startOffset}`,
    );
  output.source = source;
  output.intentText = output.intentText.replace(/\s+/g, " ").trim();
  if (!output.intentText) invalid("NousQL requires nonempty intent text");
  return output;
}
const compare = (a: string, b: string) => (a < b ? -1 : a > b ? 1 : 0);
function argumentText(value: Argument) {
  return typeof value === "number"
    ? String(value)
    : /^[a-z_][a-z0-9_]*$/.test(value) || /^\d+(ms|s|m|h|d|w)$/.test(value)
      ? value
      : JSON.stringify(value);
}
export function directiveText(d: Directive): string {
  const positional =
    d.name === "return"
      ? [...d.positional].sort(
          (a, b) =>
            resultDomainOrder.indexOf(String(a)) -
            resultDomainOrder.indexOf(String(b)),
        )
      : d.positional;
  const args = [
    ...positional.map(argumentText),
    ...Object.entries(d.named)
      .sort(([a], [b]) => compare(a, b))
      .map(([k, v]) => `${k}=${argumentText(v)}`),
  ];
  return `$${d.name}${args.length ? `(${args.join(",")})` : ""}`;
}
function atomText(atom: Atom): string {
  if (atom.kind === "text") return JSON.stringify(atom.text);
  if (atom.kind === "concept")
    return /^[\p{L}\p{N}_][\p{L}\p{N}_-]*$/u.test(atom.text)
      ? `#${atom.text}`
      : `#(${JSON.stringify(atom.text)})`;
  return `@${atom.selector}(${atom.locators.map((l) => (l.kind === "lexical" ? l.value : JSON.stringify(l.value))).join(",")})`;
}
export function canonical(syntax: QuerySyntax): string {
  const intent = syntax.intentText.replace(/[\\$@#]/g, "\\$&");
  const cues = syntax.cues.map(atomText).sort(compare);
  const directives = syntax.directives.map(directiveText).sort(compare);
  const preferences = syntax.preferences
    .map(
      (p) =>
        `$${p.negative ? "avoid" : "prefer"}(${p.operand.kind === "key" ? p.operand.value.replace(":", ",") : atomText(p.operand)})`,
    )
    .sort(compare);
  return [intent, ...cues, ...directives, ...preferences].join(" ");
}

import { createToken, EmbeddedActionsParser, Lexer } from 'chevrotain';
import { QuerySchema, type Query, type QueryInput } from '@nous-wave/contracts/cognition';
import { DomainError } from '../platform/errors.js';

const Space = createToken({ name: 'Space', pattern: /\s+/, group: Lexer.SKIPPED });
const Word = createToken({ name: 'Word', pattern: /[A-Za-z_][A-Za-z_0-9]*/ });
const keyword = (name: string) => createToken({ name, pattern: new RegExp(name, 'i'), longer_alt: Word });
const Find = keyword('FIND');
const Return = keyword('RETURN');
const Entity = keyword('ENTITY');
const Concept = keyword('CONCEPT');
const Tag = keyword('TAG');
const Time = keyword('TIME');
const In = keyword('IN');
const Between = keyword('BETWEEN');
const And = keyword('AND');
const KnowledgeAt = keyword('KNOWLEDGE_AT');
const RecordedAsOf = keyword('RECORDED_AS_OF');
const Ref = keyword('REF');
const Source = keyword('SOURCE');
const Prefer = keyword('PREFER');
const Explore = keyword('EXPLORE');
const Expand = keyword('EXPAND');
const Limit = keyword('LIMIT');
const All = keyword('ALL');
const StringLiteral = createToken({ name: 'StringLiteral', pattern: /"(?:[^"\\\r\n]|\\.)*"/ });
const Integer = createToken({ name: 'Integer', pattern: /0|[1-9][0-9]*/ });
const Left = createToken({ name: 'Left', pattern: /\[/ });
const Right = createToken({ name: 'Right', pattern: /\]/ });
const Comma = createToken({ name: 'Comma', pattern: /,/ });
const tokens = [Space, Find, Return, Entity, Concept, Tag, Time, In, Between, And,
  KnowledgeAt, RecordedAsOf, Ref, Source, Prefer, Explore, Expand, Limit, All,
  StringLiteral, Integer, Left, Right, Comma, Word];
const lexer = new Lexer(tokens);
const unquote = (text: string): string => {
  if (!text) return '';
  try { return JSON.parse(text) as string; }
  catch { throw new QuerySyntaxError('字符串需要合法的 JSON 转义', 1, 1); }
};

export class QuerySyntaxError extends DomainError {
  constructor(message: string, readonly line: number, readonly column: number) {
    super('invalid_input', `NousQL ${line}:${column}: ${message}`);
  }
}

class NousParser extends EmbeddedActionsParser {
  private seen = new Set<string>();
  private value: Record<string, unknown> = {};

  constructor() {
    super(tokens);
    this.performSelfAnalysis();
  }

  private put(key: string, value: unknown) {
    if (this.seen.has(key)) {
      const token = this.LA(0);
      throw new QuerySyntaxError(`指令 ${key} 只能出现一次`, token.startLine ?? 1, token.startColumn ?? 1);
    }
    this.seen.add(key);
    this.value[key] = value;
  }

  list = this.RULE('list', () => {
    const values: string[] = [];
    this.CONSUME(Left);
    this.OPTION(() => {
      this.AT_LEAST_ONE_SEP({
        SEP: Comma,
        DEF: () => {
          const value = this.OR([
            { ALT: () => { const token = this.CONSUME(StringLiteral); return this.ACTION(() => unquote(token.image)); } },
            { ALT: () => this.CONSUME(Word).image },
          ]);
          this.ACTION(() => values.push(value));
        },
      });
    });
    this.CONSUME(Right);
    return values;
  });

  clause = this.RULE('clause', () => {
    this.OR([
      { ALT: () => { this.CONSUME(Return); const value = this.SUBRULE(this.list); this.ACTION(() => this.put('kinds', value)); } },
      { ALT: () => { this.CONSUME(Entity); const all = this.OPTION(() => this.CONSUME(All)); const value = this.SUBRULE2(this.list); this.ACTION(() => { this.put('entities', value); this.value.entityMatch = all ? 'all' : 'any'; }); } },
      { ALT: () => { this.CONSUME(Concept); const value = this.SUBRULE3(this.list); this.ACTION(() => this.put('concepts', value)); } },
      { ALT: () => { this.CONSUME(Tag); const all = this.OPTION2(() => this.CONSUME2(All)); const value = this.SUBRULE4(this.list); this.ACTION(() => { this.put('tags', value); this.value.tagMatch = all ? 'all' : 'any'; }); } },
      { ALT: () => {
        this.CONSUME(Time); const field = this.CONSUME(Word); this.CONSUME(In);
        const clockId = this.CONSUME(StringLiteral); this.CONSUME(Between);
        const start = this.CONSUME2(StringLiteral); this.CONSUME(And);
        const end = this.CONSUME3(StringLiteral);
        this.ACTION(() => this.put('time', { field: field.image.toLowerCase(), clockId: unquote(clockId.image), start: unquote(start.image), end: unquote(end.image) }));
      } },
      { ALT: () => { this.CONSUME(KnowledgeAt); const clockId = this.CONSUME4(StringLiteral); const at = this.CONSUME5(StringLiteral); this.ACTION(() => this.put('knowledgeAt', { clockId: unquote(clockId.image), at: unquote(at.image) })); } },
      { ALT: () => { this.CONSUME(RecordedAsOf); const value = this.CONSUME6(StringLiteral); this.ACTION(() => this.put('recordedAsOf', unquote(value.image))); } },
      { ALT: () => { this.CONSUME(Ref); const value = this.SUBRULE5(this.list); this.ACTION(() => this.put('refs', value)); } },
      { ALT: () => { this.CONSUME(Source); const value = this.SUBRULE6(this.list); this.ACTION(() => this.put('sources', value)); } },
      { ALT: () => { this.CONSUME(Prefer); const value = this.SUBRULE7(this.list); this.ACTION(() => this.put('preferences', value.map(v => v.toLowerCase()))); } },
      { ALT: () => { this.CONSUME(Explore); const value = Number(this.CONSUME(Integer).image); this.ACTION(() => this.put('exploration', value)); } },
      { ALT: () => { this.CONSUME(Expand); const value = this.SUBRULE8(this.list); this.ACTION(() => this.put('expand', value.map(v => v.toLowerCase()))); } },
      { ALT: () => { this.CONSUME(Limit); const value = Number(this.CONSUME2(Integer).image); this.ACTION(() => this.put('limit', value)); } },
    ]);
  });

  query = this.RULE('query', () => {
    this.ACTION(() => { this.seen = new Set(); this.value = {}; });
    this.CONSUME(Find);
    const intent = this.CONSUME(StringLiteral);
    this.ACTION(() => { this.value.intent = unquote(intent.image); });
    this.MANY(() => this.SUBRULE(this.clause));
    return this.value;
  });
}

const parser = new NousParser();
export function resolveQuery(input: QueryInput): Query {
  if (typeof input !== 'string') return QuerySchema.parse(input);
  if (!/^\s*FIND\b/i.test(input)) return QuerySchema.parse({ intent: input });
  const lexed = lexer.tokenize(input);
  if (lexed.errors[0]) {
    const error = lexed.errors[0];
    throw new QuerySyntaxError(error.message, error.line ?? 1, error.column ?? 1);
  }
  parser.input = lexed.tokens;
  const parsed = parser.query();
  if (parser.errors[0]) {
    const error = parser.errors[0];
    throw new QuerySyntaxError(error.message, error.token.startLine ?? 1, error.token.startColumn ?? 1);
  }
  const result = QuerySchema.safeParse(parsed);
  if (!result.success) throw new QuerySyntaxError(result.error.issues.map(i => `${i.path.join('.')}: ${i.message}`).join('; '), 1, 1);
  if (result.data.time && result.data.time.end <= result.data.time.start) throw new QuerySyntaxError('TIME 的结束时间必须晚于开始时间', 1, 1);
  return result.data;
}

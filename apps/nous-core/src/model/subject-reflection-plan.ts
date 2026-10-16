import {
  MotivationRecordSchema,
  MotivationScopeSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/motivation_pb.js";
// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { create, fromJson, toJson, type JsonValue } from "@bufbuild/protobuf";
import { TimestampSchema } from "@bufbuild/protobuf/wkt";
import { Code, ConnectError } from "@connectrpc/connect";
import {
  CognitionUsageSchema,
  SourceReadScopeSchema,
  CognitiveRefSchema,
  RevisionBasisSchema,
  MemorySchema,
  type CognitiveRef,
  type RevisionBasis,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import {
  InterpretSelfRequestSchema,
  type InterpretSelfRequest,
} from "@nous-wave/protocol/nous/wave/v1alpha1/model_pb.js";
import {
  SelfRecordSchema,
  SelfScopeSchema,
  type SelfRecord,
} from "@nous-wave/protocol/nous/wave/v1alpha1/self_pb.js";
import {
  SocialRecordSchema,
  SocialScopeSchema,
} from "@nous-wave/protocol/nous/wave/v1alpha1/social_pb.js";
import {
  type SubjectReflectionBatch,
  SubjectReflectionBatchSchema,
} from "@nous-wave/protocol/nous/wave/kernel/v1alpha1/longitudinal_pb.js";
import type { KernelClient } from "../kernel-client.js";
import type { ExecutionOptions } from "../execution.js";
import {
  prepareInterpretationSource,
  prepareSelfCandidates,
  prepareSelfInterpretation,
  type SelfInterpretationPlan,
} from "./self-plan.js";
export type ReflectionPlan = {
  subjectId: string;
  domains: string[];
  basis: [string, JsonValue][];
  currentSelf: [string, JsonValue][];
  currentSocial: [string, JsonValue][];
  currentMemory: [string, JsonValue][];
  currentMotivation: [string, JsonValue][];
  timeDomainRef: string;
  entities: [string, string][];
  scopes: [string, { self: JsonValue; social: JsonValue }][];
  sourceKeys: string[];
  dependencies: CognitiveRef[];
  input: string;
  maxActions: number;
  batch?: JsonValue;
  degradation: { code: string; detail: string }[];
};
export function basisDependencies(b: RevisionBasis): CognitiveRef[] {
  if (b.basis.case === "cognitionDependency")
    return b.basis.value.targetRevision ? [b.basis.value.targetRevision] : [];
  if (b.basis.case === "seed")
    return b.basis.value.seedVersion ? [b.basis.value.seedVersion] : [];
  if (b.basis.case !== "evidence") return [];
  const e = b.basis.value;
  const refs = [
    create(CognitiveRefSchema, { kind: "occurrence", value: e.occurrenceId }),
  ];
  if (e.locator.case && e.locator.case !== "wholeOccurrence")
    refs.push(
      create(CognitiveRefSchema, {
        kind: (
          {
            sourceRegionId: "source_region",
            derivedRepresentationId: "derived_representation",
            derivedRegionId: "derived_region",
          } as const
        )[e.locator.case],
        value: e.locator.value,
      }),
    );
  return refs;
}
export function restoreSelfPlan(p: ReflectionPlan): SelfInterpretationPlan {
  return {
    input: p.input,
    basis: new Map(
      p.basis.map(([k, v]) => [k, fromJson(RevisionBasisSchema, v)]),
    ),
    current: new Map(
      p.currentSelf.map(([k, v]) => [k, fromJson(SelfRecordSchema, v)]),
    ),
    degradation: p.degradation.map((d) => ({
      $typeName: "nous.wave.v1alpha1.Degradation" as const,
      ...d,
    })),
  };
}
export async function prepareExplicitSelfPlan(
  kernel: KernelClient,
  request: InterpretSelfRequest,
  maxBytes: number,
  options: ExecutionOptions,
): Promise<ReflectionPlan> {
  const p = await prepareSelfInterpretation(kernel, request, maxBytes, options);
  return {
    subjectId: request.subjectId,
    domains: ["self"],
    basis: [...p.basis].map(([k, v]) => [k, toJson(RevisionBasisSchema, v)]),
    currentSelf: [...p.current].map(([k, v]) => [
      k,
      toJson(SelfRecordSchema, v),
    ]),
    currentSocial: [],
    currentMemory: [],
    currentMotivation: [],
    timeDomainRef: "time:real",
    entities: [],
    scopes: [
      [
        "invocation",
        {
          self: request.scope ? toJson(SelfScopeSchema, request.scope) : {},
          social: {},
        },
      ],
    ],
    sourceKeys: [...p.basis.keys()].filter(
      (k) => k === "source" || k.startsWith("source:"),
    ),
    dependencies: [...p.basis.values()].flatMap(basisDependencies),
    input: p.input,
    maxActions: 16,
    degradation: p.degradation.map((d) => ({ code: d.code, detail: d.detail })),
  };
}
export async function prepareReflectionPlan(
  kernel: KernelClient,
  batch: SubjectReflectionBatch,
  options: ExecutionOptions,
): Promise<ReflectionPlan> {
  const subjectId = batch.selected!.subjectId;
  const basis = new Map<string, RevisionBasis>(),
    sources: CognitiveRef[] = [],
    scopes: ReflectionPlan["scopes"] = [],
    entities = new Map<string, string>(),
    authorized = new Map<string, CognitiveRef>();
  const authorizedOrigins = new Set<string>();
  const sourceCatalog: unknown[] = [];
  const degradation: ReflectionPlan["degradation"] = [];
  const addRef = (r: CognitiveRef) => authorized.set(`${r.kind}:${r.value}`, r);
  const addEntity = (ref: string) => {
    if (![...entities.values()].includes(ref))
      entities.set(`entity:${entities.size}`, ref);
  };
  for (const [index, item] of batch.sources.entries()) {
    if (item.reference?.kind !== "occurrence")
      throw new ConnectError(
        "Reflection input must be actual observed occurrences",
        Code.InvalidArgument,
      );
    const request = create(InterpretSelfRequestSchema, {
      subjectId,
      source: {
        case: "observation",
        value: { occurrenceId: item.reference.value },
      },
    });
    const prepared = await prepareInterpretationSource(
      kernel,
      request,
      Math.max(
        1,
        Math.floor(Number(batch.sourceMaxBytes) / batch.sources.length),
      ),
      options,
      true,
    );
    const source = prepared.source as {
      context?: Record<string, JsonValue>;
      actor?: string;
      conversation?: string;
      partial?: boolean;
    };
    const rawUsage = source.context?.cognition_usage;
    const usage =
      rawUsage && typeof rawUsage === "object" && !Array.isArray(rawUsage)
        ? fromJson(CognitionUsageSchema, rawUsage)
        : undefined;
    if (usage?.sourceRead && usage.sourceRead.subjectId !== subjectId)
      throw new ConnectError(
        "Observed Host source scope belongs to another Subject",
        Code.PermissionDenied,
      );
    sources.push(...prepared.sources);
    prepared.sources.forEach(addRef);
    usage?.sourceRead?.sourceRefs.forEach(addRef);
    usage?.sourceRead?.originConversationRefs.forEach((origin) =>
      authorizedOrigins.add(origin),
    );
    const entityRefs = [
      ...new Set([
        ...(source.actor ? [source.actor] : []),
        ...(usage?.participants.map((p) => p.entityRef) ?? []),
        ...(usage?.audience?.recipientEntityRefs ?? []),
      ]),
    ];
    entityRefs.forEach(addEntity);
    if (usage?.audience?.groupEntityRef)
      addEntity(usage.audience.groupEntityRef);
    const conversation =
      source.conversation ?? usage?.audience?.conversationRef;
    const social = create(SocialScopeSchema, {
      description:
        "Actual captured Host observation scope; missing fields grant no broader social scope.",
      entities: entityRefs,
      conversation,
      group: usage?.audience?.groupEntityRef,
      channel: usage?.audience?.channelRef,
      domain: usage?.socialDomain,
      workContext: item.workContextId,
    });
    const self = create(SelfScopeSchema, {
      description: "Actual captured observation",
      entityRefs,
      conversationRef: conversation,
      workContextId: item.workContextId,
    });
    scopes.push([
      `source:${index}`,
      {
        self: toJson(SelfScopeSchema, self),
        social: toJson(SocialScopeSchema, social),
      },
    ]);
    for (const [key, value] of prepared.basis)
      basis.set(
        key === "source" ? `source:${index}` : `source:${index}:${key}`,
        value,
      );
    sourceCatalog.push({
      key: `source:${index}`,
      source: prepared.source,
      capturedSequence: item.recordedSequence.toString(),
      sessionId: item.sessionId,
      workContextId: item.workContextId,
      workContextRevision: item.workContextRevision?.toString(),
      scopeKey: `source:${index}`,
    });
    if (source.partial)
      degradation.push({
        code: "reflection_source_partial",
        detail: `source:${index} is a bounded partial view, never the whole source`,
      });
  }
  if (authorized.size > 1024)
    throw new ConnectError(
      "Trusted reflection source scope exceeds 1024 refs",
      Code.ResourceExhausted,
    );
  const exactContext = [...authorized.values()].filter((r) =>
    r.kind.endsWith("_revision"),
  );
  for (const [index, reference] of exactContext.entries())
    basis.set(
      `context:${index}`,
      create(RevisionBasisSchema, {
        basis: {
          case: "cognitionDependency",
          value: { targetRevision: reference, basisRole: "contextual" },
        },
      }),
    );
  const commonScope =
    scopes.length === 1
      ? fromJson(SelfScopeSchema, scopes[0]![1].self)
      : undefined;
  let selfPlan: SelfInterpretationPlan = {
    input: "",
    basis,
    current: new Map<string, SelfRecord>(),
    degradation: [],
  };
  if (batch.domains.includes("self"))
    selfPlan = await prepareSelfCandidates(
      kernel,
      create(InterpretSelfRequestSchema, {
        subjectId,
        maxCandidates: Number(batch.candidateLimit),
        scope: commonScope,
      }),
      {
        basis,
        sources,
        sourceText: JSON.stringify(sourceCatalog),
        source: sourceCatalog,
        sourceRead: create(SourceReadScopeSchema, {
          subjectId,
          sourceRefs: [...authorized.values()],
          originConversationRefs: [...authorizedOrigins],
        }),
        ranked: [...authorized.values()]
          .filter((r) =>
            ["self_facet_revision", "narrative_thread_revision"].includes(
              r.kind,
            ),
          )
          .slice(0, 128),
      },
      Number(batch.candidateMaxBytes) * 4,
      options,
    );
  for (const [k, v] of selfPlan.basis) basis.set(k, v);
  const socialCurrent: ReflectionPlan["currentSocial"] = [];
  let socialBytes = Number(batch.candidateMaxBytes);
  if (batch.domains.includes("social"))
    for (const [, s] of scopes) {
      if (
        socialCurrent.length >= Number(batch.candidateLimit) ||
        socialBytes <= 0
      ) {
        degradation.push({
          code: "reflection_social_candidates_omitted",
          detail:
            "Additional actual scopes exceed this opportunity's candidate budget",
        });
        break;
      }
      const selected = await kernel.social.select(
        {
          subjectId,
          scope: fromJson(SocialScopeSchema, s.social),
          sourceRead: {
            subjectId,
            sourceRefs: [...authorized.values()],
            originConversationRefs: [...authorizedOrigins],
          },
          maxItems: Number(batch.candidateLimit) - socialCurrent.length,
          maxBytes: socialBytes,
        },
        options,
      );
      for (const record of selected.items) {
        if (
          socialCurrent.some(
            ([, v]) =>
              (v as { reference?: { value?: string } }).reference?.value ===
              record.reference?.value,
          )
        )
          continue;
        const json = toJson(SocialRecordSchema, record);
        socialBytes -= Buffer.byteLength(JSON.stringify(json));
        const key = `social:${socialCurrent.length}`;
        socialCurrent.push([key, json]);
        if (record.reference)
          basis.set(
            key,
            create(RevisionBasisSchema, {
              basis: {
                case: "cognitionDependency",
                value: {
                  targetRevision: record.reference,
                  basisRole: "interpretation",
                },
              },
            }),
          );
      }
      if (selected.omitted)
        degradation.push({
          code: "reflection_social_candidates_omitted",
          detail:
            "Authorized scoped Social candidates exceed bounded complete read; unselected matters may exist",
        });
    }
  const motivationCurrent: ReflectionPlan["currentMotivation"] = [];
  let motivationBytes = Number(batch.candidateMaxBytes);
  if (batch.domains.includes("motivation"))
    for (const [, s] of scopes) {
      if (
        motivationCurrent.length >= Number(batch.candidateLimit) ||
        motivationBytes <= 0
      ) {
        degradation.push({
          code: "reflection_motivation_candidates_omitted",
          detail:
            "Additional actual scopes exceed this opportunity's candidate budget",
        });
        break;
      }
      const selected = await kernel.motivation.select(
        {
          subjectId,
          scope: fromJson(MotivationScopeSchema, s.social),
          sourceRead: { subjectId, sourceRefs: [...authorized.values()] },
          maxItems: Number(batch.candidateLimit) - motivationCurrent.length,
          maxBytes: motivationBytes,
        },
        options,
      );
      for (const record of selected.items) {
        if (
          motivationCurrent.some(
            ([, v]) =>
              (v as { reference?: { value?: string } }).reference?.value ===
              record.reference?.value,
          )
        )
          continue;
        const json = toJson(MotivationRecordSchema, record);
        motivationBytes -= Buffer.byteLength(JSON.stringify(json));
        const key = `motivation:${motivationCurrent.length}`;
        motivationCurrent.push([key, json]);
        if (record.reference)
          basis.set(
            key,
            create(RevisionBasisSchema, {
              basis: {
                case: "cognitionDependency",
                value: {
                  targetRevision: record.reference,
                  basisRole: "interpretation",
                },
              },
            }),
          );
      }
      if (selected.omitted)
        degradation.push({
          code: "reflection_motivation_candidates_omitted",
          detail:
            "Authorized scoped Motivation candidates exceed bounded complete read; unselected matters may exist",
        });
    }
  const memoryCurrent: ReflectionPlan["currentMemory"] = [];
  if (batch.domains.includes("memory")) {
    const current = await kernel.maintenance.reflectionMemoryCandidates(
      {
        subjectId,
        references: [...authorized.values()],
        maxItems: Number(batch.candidateLimit),
        maxBytes: batch.candidateMaxBytes,
      },
      options,
    );
    for (const record of current.items) {
      const key = `memory:${memoryCurrent.length}`;
      memoryCurrent.push([key, toJson(MemorySchema, record)]);
      basis.set(
        key,
        create(RevisionBasisSchema, {
          basis: {
            case: "cognitionDependency",
            value: {
              targetRevision: {
                kind: "memory_revision",
                value: record.revisionId,
              },
              basisRole: "interpretation",
            },
          },
        }),
      );
    }
    if (current.omitted)
      degradation.push({
        code: "reflection_memory_candidates_omitted",
        detail:
          "Complete current actual Memory anchors exceed candidate budget",
      });
  }
  const plan: ReflectionPlan = {
    subjectId,
    domains: batch.domains,
    basis: [...basis].map(([k, v]) => [k, toJson(RevisionBasisSchema, v)]),
    currentSelf: [...selfPlan.current].map(([k, v]) => [
      k,
      toJson(SelfRecordSchema, v),
    ]),
    currentSocial: socialCurrent,
    currentMemory: memoryCurrent,
    currentMotivation: motivationCurrent,
    timeDomainRef: batch.domains.includes("motivation")
      ? (await kernel.clocks.readClock({ subjectId }, options)).timeDomainRef
      : "time:real",
    entities: [...entities],
    scopes,
    sourceKeys: [...basis.keys()].filter((k) => k.startsWith("source:")),
    dependencies: [...basis.values()].flatMap(basisDependencies),
    input: "",
    maxActions: Number(batch.maxActions),
    batch: toJson(SubjectReflectionBatchSchema, batch),
    degradation: [
      ...degradation,
      ...selfPlan.degradation.map((d) => ({ code: d.code, detail: d.detail })),
    ],
  };
  plan.input = JSON.stringify({
    actualSources: sourceCatalog,
    actualSourceChanges: batch.changes.map((c) => ({
      changeId: c.changeId.toString(),
      reference: c.reference,
      reason: c.reason,
      recordedAt: c.recordedAt
        ? toJson(TimestampSchema, c.recordedAt)
        : undefined,
      externalEvidence: false,
    })),
    enabledDomains: plan.domains,
    actualTimeDomain: plan.timeDomainRef,
    sourceRead: {
      subjectId,
      sourceRefs: [...authorized.values()].map((r) =>
        toJson(CognitiveRefSchema, r),
      ),
    },
    entityCatalog: plan.entities.map(([key, entityRef]) => ({
      key,
      entityRef,
    })),
    scopeCatalog: plan.scopes.map(([key, scope]) => ({ key, ...scope })),
    exactContextCatalog: exactContext.map((reference, index) => ({
      key: `context:${index}`,
      reference: toJson(CognitiveRefSchema, reference),
      access: "authorized exact reference; no unseen body claimed",
    })),
    basisCatalog: plan.basis.map(([key, value]) => ({ key, basis: value })),
    currentSelf: plan.currentSelf.map(([key, record]) => ({ key, record })),
    currentSocial: plan.currentSocial.map(([key, record]) => ({ key, record })),
    currentMotivation: plan.currentMotivation.map(([key, record]) => ({
      key,
      record,
    })),
    currentMemory: plan.currentMemory.map(([key, record]) => ({ key, record })),
    candidateScope: {
      completeSubjectSnapshot: false,
      memory:
        "Only current exact Memory anchors actually supplied by the trusted invocation; no global body scan",
      omitted: plan.degradation,
      remainingSources: batch.more,
      maxActions: plan.maxActions,
    },
  });
  if (Buffer.byteLength(JSON.stringify(plan)) > 4 * 1024 * 1024)
    throw new ConnectError(
      "Shared reflection prepared source/catalog exceeds 4MiB immutable result budget",
      Code.ResourceExhausted,
    );
  return plan;
}

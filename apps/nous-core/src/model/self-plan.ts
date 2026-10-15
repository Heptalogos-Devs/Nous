// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import { create, toJson } from "@bufbuild/protobuf";
import { timestampDate, timestampFromDate } from "@bufbuild/protobuf/wkt";
import { Code, ConnectError } from "@connectrpc/connect";
import {
  RevisionBasisSchema,
  TemporalExtentSchema,
  type CognitiveRef,
  CognitiveRefSchema,
  type ProducerSignature,
  type RevisionBasis,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import {
  SelfContentSchema,
  SelfRecordSchema,
  type SelfRecord,
} from "@nous-wave/protocol/nous/wave/v1alpha1/self_pb.js";
import type { InterpretSelfRequest } from "@nous-wave/protocol/nous/wave/v1alpha1/model_pb.js";
import type { KernelClient } from "../kernel-client.js";
import type { ExecutionOptions } from "../execution.js";
import type { SelfInterpretationProposal } from "./schemas/self.js";

type Action = SelfInterpretationProposal["actions"][number];
type Content = Extract<Action, { action: "adopt" }>["content"];
export function invalidSelf(message: string): never {
  throw new ConnectError(message, Code.InvalidArgument);
}
export async function prepareSelfInterpretation(
  kernel: KernelClient,
  request: InterpretSelfRequest,
  maxBytes: number,
  options: ExecutionOptions,
) {
  const basis = new Map<string, RevisionBasis>();
  const current = new Map<string, SelfRecord>();
  const subjectId = request.subjectId;
  const sources: CognitiveRef[] = [];
  let sourceText = "";
  let source: unknown;
  if (request.source.case === "seedVersionId") {
    const seed = await kernel.subjects.getCognitiveSeedVersion(
      { subjectId, id: request.source.value },
      options,
    );
    const text = seed.seed?.text ?? "";
    sourceText = text;
    sources.push(
      create(CognitiveRefSchema, {
        kind: "cognitive_seed_version",
        value: seed.seedVersionId,
      }),
    );
    if (!text.trim()) invalidSelf("Seed text is empty");
    if (Buffer.byteLength(text) > maxBytes)
      throw new ConnectError(
        "Seed exceeds interpretation source budget",
        Code.ResourceExhausted,
      );
    const entry = (semanticPath: string) =>
      create(RevisionBasisSchema, {
        basis: {
          case: "seed",
          value: {
            seedVersion: {
              kind: "cognitive_seed_version",
              value: seed.seedVersionId,
            },
            semanticPath,
          },
        },
      });
    basis.set("source", entry("$"));
    text
      .split("\n")
      .forEach((_, index) =>
        basis.set(`source:line:${index + 1}`, entry(`line:${index + 1}`)),
      );
    source = {
      kind: "adopted_seed",
      text,
      format: seed.format,
      provenance: seed.seed?.provenance,
    };
  } else if (request.source.case === "observation") {
    const observation = request.source.value;
    const occurrence = await kernel.material.getOccurrence(
      { subjectId, id: observation.occurrenceId },
      options,
    );
    const prepared = await kernel.materialWorkflow.prepareFormation(
      {
        subjectId,
        occurrenceId: observation.occurrenceId,
        representationId: observation.representationId,
        maxBytes: BigInt(maxBytes),
      },
      options,
    );
    const material = prepared.source;
    if (!material?.reference)
      throw new ConnectError("Self evidence is unavailable", Code.NotFound);
    if (material.partial)
      throw new ConnectError(
        "Self evidence exceeds source budget; select a bounded representation",
        Code.ResourceExhausted,
      );
    const mime = material.mediaType.split(";")[0]!.toLowerCase();
    if (!mime.startsWith("text/") && mime !== "application/json")
      throw new ConnectError(
        "Self reflection requires a textual representation",
        Code.FailedPrecondition,
      );
    const text = new TextDecoder("utf-8", { fatal: true }).decode(
      material.content,
    );
    sourceText = text;
    sources.push(
      create(CognitiveRefSchema, {
        kind: "occurrence",
        value: observation.occurrenceId,
      }),
      material.reference,
    );
    basis.set(
      "source",
      create(RevisionBasisSchema, {
        basis: {
          case: "evidence",
          value: {
            occurrenceId: observation.occurrenceId,
            locator:
              material.reference.kind === "derived_representation"
                ? {
                    case: "derivedRepresentationId",
                    value: material.reference.value,
                  }
                : { case: "wholeOccurrence", value: true },
            basisRole: "direct",
          },
        },
      }),
    );
    source = {
      kind: "observed_material",
      evidenceText: text,
      sourceClass: occurrence.sourceClass,
      actor: occurrence.actorEntityRef,
      conversation: occurrence.conversationRef,
      context: occurrence.context,
      occurredTime: occurrence.occurredTime
        ? toJson(TemporalExtentSchema, occurrence.occurredTime)
        : undefined,
      observedAt: occurrence.observedAt
        ? timestampDate(occurrence.observedAt).toISOString()
        : undefined,
    };
  } else
    invalidSelf(
      "Self interpretation requires an adopted Seed or actual observation",
    );
  const maxCandidates = request.maxCandidates || 32;
  if (maxCandidates > 128 || request.targets.length > maxCandidates)
    invalidSelf(
      "Self candidate budget must be 1..128 and contain explicit targets",
    );
  const intent = Array.from(sourceText).slice(0, 2048).join("");
  const selection = await kernel.self.select(
    {
      subjectId,
      scope: request.scope,
      targets: request.targets,
      sources,
      intent,
      maxItems: maxCandidates,
      maxBytes: Math.floor(maxBytes / 4),
    },
    options,
  );
  const degradation = selection.degradation;
  for (const record of selection.items) {
    if (
      record.acceptanceState !== "accepted" ||
      record.suppressionState !== "normal"
    )
      continue;
    const key = `self:${current.size}`;
    current.set(key, record);
    const content = record.content?.content;
    if (content?.case) {
      content.value.basis.forEach((entry, index) =>
        basis.set(`${key}:basis:${index}`, entry),
      );
      record.evidence.forEach((entry, index) => {
        if (entry.basis) basis.set(`${key}:evidence:${index}`, entry.basis);
      });
      if (content.case === "facet")
        content.value.exceptions.forEach((exception, index) =>
          exception.basis.forEach((entry, basisIndex) =>
            basis.set(`${key}:exception:${index}:${basisIndex}`, entry),
          ),
        );
    }
    if (record.integrityState === "valid")
      basis.set(
        key,
        create(RevisionBasisSchema, {
          basis: {
            case: "cognitionDependency",
            value: { targetRevision: record.reference, basisRole: "direct" },
          },
        }),
      );
  }
  const candidateScope = {
    completeSubjectSnapshot: false,
    selection: selection.candidateScope,
    countBudget: maxCandidates,
    bodyAndSupportByteBudget: Math.floor(maxBytes / 4),
    retrievalSourceCharacters: Array.from(intent).length,
    retrievalSourceTruncated: intent !== sourceText,
    invocationScope: request.scope,
    omitted: degradation,
    interpretation:
      "Unselected Self may exist. Candidate absence is not proof of absence or inapplicability. Insufficient evidence for a new-vs-existing decision requires an exact target follow-up, not an exhaustive Self read.",
  };
  const input = JSON.stringify(
    {
      source,
      candidateScope,
      invocationScope: request.scope,
      basisCatalog: [...basis].map(([key, entry]) => ({
        key,
        basis: toJson(RevisionBasisSchema, entry),
      })),
      currentSelf: [...current].map(([key, record]) => ({
        key,
        record: toJson(SelfRecordSchema, record),
      })),
    },
    (_key, value: unknown) =>
      typeof value === "bigint" ? value.toString() : value,
  );
  if (Buffer.byteLength(input) > maxBytes * 2)
    throw new ConnectError(
      "Self interpretation snapshot exceeds input budget",
      Code.ResourceExhausted,
    );
  return { input, basis, current, degradation };
}
export type SelfInterpretationPlan = Awaited<
  ReturnType<typeof prepareSelfInterpretation>
>;
export function selectedSelfBasis(
  plan: SelfInterpretationPlan,
  keys: string[],
  targetKey?: string,
  requireNewSource = true,
) {
  if (new Set(keys).size !== keys.length || keys.includes(targetKey ?? ""))
    invalidSelf("Duplicate or self-dependent Self basis");
  if (
    requireNewSource &&
    !keys.some((key) => key === "source" || key.startsWith("source:line:"))
  )
    invalidSelf("A Self change must cite its new source");
  return keys.map(
    (key) =>
      plan.basis.get(key) ??
      invalidSelf("Self basis key is outside supplied available catalog"),
  );
}
export function selectedSelfTarget(plan: SelfInterpretationPlan, key: string) {
  return (
    plan.current.get(key) ??
    invalidSelf("Self target key is outside supplied snapshot")
  );
}
export function proposedSelfContent(
  plan: SelfInterpretationPlan,
  request: InterpretSelfRequest,
  content: Content,
  producer: ProducerSignature,
  targetKey?: string,
) {
  const basis = selectedSelfBasis(plan, content.basisKeys, targetKey);
  const temporal = content.validTime;
  const validTime = create(TemporalExtentSchema, {
    value:
      temporal.kind === "unknown"
        ? undefined
        : temporal.kind === "instant"
          ? { case: "instant", value: timestampFromDate(new Date(temporal.at)) }
          : {
              case: "interval",
              value: {
                start:
                  temporal.start === null
                    ? undefined
                    : timestampFromDate(new Date(temporal.start)),
                end:
                  temporal.end === null
                    ? undefined
                    : timestampFromDate(new Date(temporal.end)),
              },
            },
  });
  if (content.family === "self_facet")
    return create(SelfContentSchema, {
      content: {
        case: "facet",
        value: {
          kind: content.kind,
          statement: content.statement,
          stance: content.stance,
          epistemicClass: content.epistemicClass,
          scope: {
            description: content.scope.description,
            purposes: content.scope.purposes,
            ...(content.scope.bindToInvocation
              ? {
                  entityRefs: request.scope?.entityRefs,
                  workContextId: request.scope?.workContextId,
                  conversationRef: request.scope?.conversationRef,
                }
              : {}),
          },
          exceptions: content.exceptions.map((exception) => ({
            statement: exception.statement,
            basis: selectedSelfBasis(
              plan,
              exception.basisKeys,
              targetKey,
              false,
            ),
          })),
          basis,
          validTime,
          producer,
        },
      },
    });
  if (
    new Set(content.sourceKeys).size !== content.sourceKeys.length ||
    content.sourceKeys.includes(targetKey ?? "")
  )
    invalidSelf("Duplicate or self-dependent narrative sources");
  const sourceRefs = content.sourceKeys.map((key) => {
    if (!plan.basis.has(key)) invalidSelf("Narrative source is unavailable");
    return selectedSelfTarget(plan, key).reference!;
  });
  return create(SelfContentSchema, {
    content: {
      case: "narrative",
      value: {
        theme: content.theme,
        interpretation: content.interpretation,
        epistemicClass: content.epistemicClass,
        sourceRefs,
        basis,
        validTime,
        producer,
      },
    },
  });
}

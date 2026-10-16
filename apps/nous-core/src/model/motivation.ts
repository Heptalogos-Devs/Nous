// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
import {
  create,
  fromJson,
  toJson,
  type JsonValue,
  type MessageInitShape,
} from "@bufbuild/protobuf";
import {
  MotivationRecordSchema,
  MotivationScopeSchema,
  MotivationContentSchema,
  AdoptMotivationRequestSchema,
  ReviseMotivationRequestSchema,
  AddMotivationEvidenceRequestSchema,
  CriterionExpressionSchema,
  type CriterionExpression,
} from "@nous-wave/protocol/nous/wave/v1alpha1/motivation_pb.js";
import { Code, ConnectError } from "@connectrpc/connect";
import {
  RevisionBasisSchema,
  type ProducerSignature,
} from "@nous-wave/protocol/nous/wave/v1alpha1/types_pb.js";
import type { ReflectionPlan } from "./subject-reflection-plan.js";
import { basisDependencies } from "./subject-reflection-plan.js";
import { chosenBasis, temporal } from "./reflection-catalog.js";
import type {
  MotivationAction,
  CriterionBlueprint,
} from "./schemas/motivation.js";
function invalid(message: string): never {
  throw new ConnectError(message, Code.InvalidArgument);
}
export function compileMotivationAction(
  plan: ReflectionPlan,
  action: MotivationAction,
  scopeJson: JsonValue | undefined,
  operationId: string,
  producer: ProducerSignature,
) {
  const current = new Map(
    plan.currentMotivation.map(([key, json]) => [
      key,
      fromJson(MotivationRecordSchema, json),
    ]),
  );
  const targetKey = "targetKey" in action ? action.targetKey : undefined;
  const target = targetKey ? current.get(targetKey) : undefined;
  if (targetKey && !target)
    invalid("Motivation target must be an exact current candidate");
  if (action.action === "keep") return {};
  const mutation = {
    subjectId: plan.subjectId,
    operationId,
    targetRevision: target?.reference,
    expectedObjectEpoch: target?.objectEpoch,
  };
  if (action.action === "add_evidence")
    return {
      request: toJson(
        AddMotivationEvidenceRequestSchema,
        create(AddMotivationEvidenceRequestSchema, {
          ...mutation,
          role: action.role,
          basis: chosenBasis(
            plan,
            [action.basisKey],
            targetKey ?? undefined,
          )[0],
        }),
      ),
      targetObject: target!.objectRef,
    };
  const c = action.content;
  const exactRef = (key: string) => {
    const basis = new Map(plan.basis).get(key);
    if (!basis)
      invalid("Criterion source/target is outside actual reflection catalog");
    const deps = basisDependencies(fromJson(RevisionBasisSchema, basis));
    return deps[0] ?? invalid("Criterion key has no actual reference");
  };
  function compile(v: CriterionBlueprint, depth = 1): CriterionExpression {
    if (depth > 32) invalid("Reflection criterion exceeds bounded depth");
    const expression: MessageInitShape<
      typeof CriterionExpressionSchema
    >["expression"] =
      v.kind === "eventWitness"
        ? {
            case: "eventWitness",
            value: {
              version: 1,
              eventType: v.eventType,
              target: v.target ?? undefined,
              activity: v.activity ?? undefined,
            },
          }
        : v.kind === "ownerState"
          ? {
              case: "ownerState",
              value: {
                version: 1,
                reference: exactRef(v.targetKey),
                state: v.state,
              },
            }
          : v.kind === "measurement"
            ? {
                case: "measurement",
                value: {
                  version: 1,
                  name: v.name,
                  unit: v.unit,
                  comparison: v.comparison,
                  value: v.value,
                },
              }
            : v.kind === "relation"
              ? {
                  case: "relation",
                  value: {
                    version: 1,
                    reference: exactRef(v.targetKey),
                    relationType: v.relationType,
                    degree: v.degree ?? undefined,
                  },
                }
              : v.kind === "semantic"
                ? {
                    case: "semantic",
                    value: {
                      version: 1,
                      question: v.question,
                      materialRefs: v.materialKeys.map(exactRef),
                    },
                  }
                : v.kind === "not"
                  ? { case: "not", value: compile(v.item, depth + 1) }
                  : {
                      case: v.kind,
                      value: {
                        items: v.items.map((i) => compile(i, depth + 1)),
                      },
                    };
    return create(CriterionExpressionSchema, { expression });
  }
  if (
    c.condition &&
    ![plan.timeDomainRef, "time:real"].includes(c.condition.timeDomainRef)
  )
    invalid(
      "Motivation time domain was not present in the actual clock catalog",
    );
  const content = create(MotivationContentSchema, {
    family: c.family,
    matterKey: c.matterKey,
    statement: c.statement,
    scope: scopeJson ? fromJson(MotivationScopeSchema, scopeJson) : undefined,
    basis: chosenBasis(plan, c.basisKeys, targetKey ?? undefined),
    validTime: temporal(c.validTime),
    producer,
    links: c.links.map((l) => ({
      relation: l.relation,
      targetRevision:
        current.get(l.targetKey)?.reference ??
        invalid("Motivation link is outside current exact catalog"),
    })),
    condition: c.condition
      ? {
          expression: compile(c.condition.expression),
          temporalForm: c.condition.temporalForm,
          window: {
            timeDomainRef: c.condition.timeDomainRef,
            extent: temporal(c.condition.window),
          },
          coverageRequirement: c.condition.coverageRequirement,
          holdSeconds:
            c.condition.holdSeconds === null
              ? undefined
              : BigInt(c.condition.holdSeconds),
        }
      : undefined,
  });
  return {
    request:
      action.action === "adopt"
        ? toJson(
            AdoptMotivationRequestSchema,
            create(AdoptMotivationRequestSchema, {
              ...mutation,
              content,
              reason: action.reason,
            }),
          )
        : toJson(
            ReviseMotivationRequestSchema,
            create(ReviseMotivationRequestSchema, {
              ...mutation,
              content,
              reason: action.reason,
            }),
          ),
    targetObject: target?.objectRef,
  };
}

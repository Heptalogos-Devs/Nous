/* tslint:disable */
/* eslint-disable */
/**
 * 
 * @export
 * @interface Actor
 */
export interface Actor {
    /**
     * 
     */
    kind: ActorKindEnum;
    /**
     * 
     */
    id: string;
    /**
     * 
     */
    grants: Array<string>;
    /**
     * 
     */
    subjectIds?: Array<string>;
    /**
     * 
     */
    expiresAt?: string;
    /**
     * 
     */
    consumer?: Consumer;
}


/**
 * @export
 */
export const ActorKindEnum = {
    Human: 'human',
    Subject: 'subject',
    Service: 'service',
    Worker: 'worker',
    Author: 'author',
} as const;
export type ActorKindEnum = typeof ActorKindEnum[keyof typeof ActorKindEnum];

/**
 * 
 * @export
 * @interface ActorInput
 */
export interface ActorInput {
    /**
     * 
     */
    kind: ActorInputKindEnum;
    /**
     * 
     */
    id: string;
    /**
     * 
     */
    grants: Array<string>;
    /**
     * 
     */
    subjectIds?: Array<string>;
    /**
     * 
     */
    expiresAt?: string;
    /**
     * 
     */
    consumer?: ConsumerInput;
}


/**
 * @export
 */
export const ActorInputKindEnum = {
    Human: 'human',
    Subject: 'subject',
    Service: 'service',
    Worker: 'worker',
    Author: 'author',
} as const;
export type ActorInputKindEnum = typeof ActorInputKindEnum[keyof typeof ActorInputKindEnum];

/**
 * 
 * @export
 * @interface CallContext
 */
export interface CallContext {
    /**
     * 
     */
    actor: Actor;
    /**
     * 
     */
    subjectId: string;
    /**
     * 
     */
    purpose: string;
    /**
     * 
     */
    audience: Array<string>;
    /**
     * 
     */
    consumer?: Consumer;
    /**
     * 
     */
    traceContext: CallContextTraceContext;
}
/**
 * 
 * @export
 * @interface CallContextInput
 */
export interface CallContextInput {
    /**
     * 
     */
    actor: ActorInput;
    /**
     * 
     */
    subjectId: string;
    /**
     * 
     */
    purpose: string;
    /**
     * 
     */
    audience: Array<string>;
    /**
     * 
     */
    consumer?: ConsumerInput;
    /**
     * 
     */
    traceContext: CallContextInputTraceContext;
}
/**
 * 
 * @export
 * @interface CallContextInputTraceContext
 */
export interface CallContextInputTraceContext {
    /**
     * 
     */
    traceId: string;
    /**
     * 
     */
    operationId: string;
    /**
     * 
     */
    traceparent?: string;
}
/**
 * 
 * @export
 * @interface CallContextTraceContext
 */
export interface CallContextTraceContext {
    /**
     * 
     */
    traceId: string;
    /**
     * 
     */
    operationId: string;
    /**
     * 
     */
    traceparent?: string;
}
/**
 * 
 * @export
 * @interface ChangeSet
 */
export interface ChangeSet {
    /**
     * 
     */
    groups: Array<ChangeSetGroupsInner>;
}
/**
 * 
 * @export
 * @interface ChangeSetGroupsInner
 */
export interface ChangeSetGroupsInner {
    /**
     * 
     */
    id: string;
    /**
     * 
     */
    reason: string;
    /**
     * 
     */
    changes: Array<KnowledgeChange>;
}
/**
 * 
 * @export
 * @interface ChangeSetInput
 */
export interface ChangeSetInput {
    /**
     * 
     */
    groups: Array<GrowthResolveRequestGroupsInner>;
}
/**
 * 
 * @export
 * @interface CognitiveSchema
 */
export interface CognitiveSchema {
    /**
     * 
     */
    content: string;
    /**
     * 
     */
    title?: string;
    /**
     * 
     */
    entityRefs: Array<Ref>;
    /**
     * 
     */
    conceptRefs: Array<Ref>;
    /**
     * 
     */
    tags: Array<string>;
    /**
     * 
     */
    kind: CognitiveSchemaKindEnum;
    /**
     * 
     */
    context: string;
    /**
     * 
     */
    supports: Array<Ref>;
    /**
     * 
     */
    exceptions: Array<Ref>;
    /**
     * 
     */
    openQuestions: Array<string>;
    /**
     * 
     */
    prediction?: string;
}


/**
 * @export
 */
export const CognitiveSchemaKindEnum = {
    Schema: 'Schema',
} as const;
export type CognitiveSchemaKindEnum = typeof CognitiveSchemaKindEnum[keyof typeof CognitiveSchemaKindEnum];

/**
 * 
 * @export
 * @interface CognitiveSchemaInput
 */
export interface CognitiveSchemaInput {
    /**
     * 
     */
    content: string;
    /**
     * 
     */
    title?: string;
    /**
     * 
     */
    entityRefs?: Array<RefInput>;
    /**
     * 
     */
    conceptRefs?: Array<RefInput>;
    /**
     * 
     */
    tags?: Array<string>;
    /**
     * 
     */
    kind: CognitiveSchemaInputKindEnum;
    /**
     * 
     */
    context: string;
    /**
     * 
     */
    supports: Array<RefInput>;
    /**
     * 
     */
    exceptions: Array<RefInput>;
    /**
     * 
     */
    openQuestions?: Array<string>;
    /**
     * 
     */
    prediction?: string;
}


/**
 * @export
 */
export const CognitiveSchemaInputKindEnum = {
    Schema: 'Schema',
} as const;
export type CognitiveSchemaInputKindEnum = typeof CognitiveSchemaInputKindEnum[keyof typeof CognitiveSchemaInputKindEnum];

/**
 * 
 * @export
 * @interface Concept
 */
export interface Concept {
    /**
     * 
     */
    content: string;
    /**
     * 
     */
    title?: string;
    /**
     * 
     */
    entityRefs: Array<Ref>;
    /**
     * 
     */
    conceptRefs: Array<Ref>;
    /**
     * 
     */
    tags: Array<string>;
    /**
     * 
     */
    kind: ConceptKindEnum;
    /**
     * 
     */
    name: string;
    /**
     * 
     */
    aliases: Array<string>;
    /**
     * 
     */
    language: string;
}


/**
 * @export
 */
export const ConceptKindEnum = {
    Concept: 'Concept',
} as const;
export type ConceptKindEnum = typeof ConceptKindEnum[keyof typeof ConceptKindEnum];

/**
 * 
 * @export
 * @interface ConceptInput
 */
export interface ConceptInput {
    /**
     * 
     */
    content: string;
    /**
     * 
     */
    title?: string;
    /**
     * 
     */
    entityRefs?: Array<RefInput>;
    /**
     * 
     */
    conceptRefs?: Array<RefInput>;
    /**
     * 
     */
    tags?: Array<string>;
    /**
     * 
     */
    kind: ConceptInputKindEnum;
    /**
     * 
     */
    name: string;
    /**
     * 
     */
    aliases?: Array<string>;
    /**
     * 
     */
    language?: string;
}


/**
 * @export
 */
export const ConceptInputKindEnum = {
    Concept: 'Concept',
} as const;
export type ConceptInputKindEnum = typeof ConceptInputKindEnum[keyof typeof ConceptInputKindEnum];

/**
 * 
 * @export
 * @interface ConceptsMergeRequest
 */
export interface ConceptsMergeRequest {
    /**
     * 
     */
    kept: EpisodesOrganizeRequestEpisodesInnerTarget;
    /**
     * 
     */
    merged: Array<EpisodesOrganizeRequestEpisodesInnerTarget>;
    /**
     * 
     */
    content?: string;
    /**
     * 
     */
    formed: TimePointInput;
    /**
     * 
     */
    reason: string;
}
/**
 * 
 * @export
 * @interface ConceptsSplitRequest
 */
export interface ConceptsSplitRequest {
    /**
     * 
     */
    source: EpisodesOrganizeRequestEpisodesInnerTarget;
    /**
     * 
     */
    branches: Array<ConceptsSplitRequestBranchesInner>;
    /**
     * 
     */
    remainingDefinition?: string;
    /**
     * 
     */
    formed: TimePointInput;
    /**
     * 
     */
    reason: string;
}
/**
 * 
 * @export
 * @interface ConceptsSplitRequestBranchesInner
 */
export interface ConceptsSplitRequestBranchesInner {
    /**
     * 
     */
    name: string;
    /**
     * 
     */
    aliases?: Array<string>;
    /**
     * 
     */
    definition: string;
    /**
     * 
     */
    memberRefs: Array<RefInput>;
}
/**
 * 
 * @export
 * @interface Consumer
 */
export interface Consumer {
    /**
     * 
     */
    kind: ConsumerKindEnum;
    /**
     * 
     */
    host: string;
    /**
     * 
     */
    id: string;
}


/**
 * @export
 */
export const ConsumerKindEnum = {
    Subject: 'subject',
    Worker: 'worker',
    Author: 'author',
    Maintenance: 'maintenance',
} as const;
export type ConsumerKindEnum = typeof ConsumerKindEnum[keyof typeof ConsumerKindEnum];

/**
 * 
 * @export
 * @interface ConsumerInput
 */
export interface ConsumerInput {
    /**
     * 
     */
    kind: ConsumerInputKindEnum;
    /**
     * 
     */
    host: string;
    /**
     * 
     */
    id: string;
}


/**
 * @export
 */
export const ConsumerInputKindEnum = {
    Subject: 'subject',
    Worker: 'worker',
    Author: 'author',
    Maintenance: 'maintenance',
} as const;
export type ConsumerInputKindEnum = typeof ConsumerInputKindEnum[keyof typeof ConsumerInputKindEnum];

/**
 * 
 * @export
 * @interface ContextBlock
 */
export interface ContextBlock {
    /**
     * 
     */
    kind: ContextBlockKindEnum;
    /**
     * 
     */
    text: string;
    /**
     * 
     */
    refs: Array<Ref>;
    /**
     * 
     */
    tokens: number;
}


/**
 * @export
 */
export const ContextBlockKindEnum = {
    Task: 'task',
    Identity: 'identity',
    Situation: 'situation',
    Knowledge: 'knowledge',
    Sources: 'sources',
    Work: 'work',
} as const;
export type ContextBlockKindEnum = typeof ContextBlockKindEnum[keyof typeof ContextBlockKindEnum];

/**
 * 
 * @export
 * @interface ContextBlockInput
 */
export interface ContextBlockInput {
    /**
     * 
     */
    kind: ContextBlockInputKindEnum;
    /**
     * 
     */
    text: string;
    /**
     * 
     */
    refs: Array<RefInput>;
    /**
     * 
     */
    tokens: number;
}


/**
 * @export
 */
export const ContextBlockInputKindEnum = {
    Task: 'task',
    Identity: 'identity',
    Situation: 'situation',
    Knowledge: 'knowledge',
    Sources: 'sources',
    Work: 'work',
} as const;
export type ContextBlockInputKindEnum = typeof ContextBlockInputKindEnum[keyof typeof ContextBlockInputKindEnum];

/**
 * 
 * @export
 * @interface ContextPrepare
 */
export interface ContextPrepare {
    /**
     * 
     */
    consumer: Consumer;
    /**
     * 
     */
    role: string;
    /**
     * 
     */
    workContextId?: string;
    /**
     * 
     */
    roleTask: string;
    /**
     * 
     */
    situation: string;
    /**
     * 
     */
    query?: string;
    /**
     * 
     */
    latestMaterials: Array<InputPortion>;
    /**
     * 
     */
    requiredSources: Array<Ref>;
    /**
     * 
     */
    model: ContextPrepareModel;
    /**
     * 
     */
    budget: ContextPrepareBudget;
    /**
     * 
     */
    observed: TimePoint;
}
/**
 * 
 * @export
 * @interface ContextPrepareBudget
 */
export interface ContextPrepareBudget {
    /**
     * 
     */
    outputTokens: number;
    /**
     * 
     */
    toolTokens: number;
    /**
     * 
     */
    requiredTokens: number;
    /**
     * 
     */
    mediaTokens: number;
    /**
     * 
     */
    cognitionTokens?: number;
}
/**
 * 
 * @export
 * @interface ContextPrepareInput
 */
export interface ContextPrepareInput {
    /**
     * 
     */
    consumer: ConsumerInput;
    /**
     * 
     */
    role?: string;
    /**
     * 
     */
    workContextId?: string;
    /**
     * 
     */
    roleTask?: string;
    /**
     * 
     */
    situation?: string;
    /**
     * 
     */
    query?: string;
    /**
     * 
     */
    latestMaterials?: Array<InputPortionInput>;
    /**
     * 
     */
    requiredSources?: Array<RefInput>;
    /**
     * 
     */
    model: ContextPrepareInputModel;
    /**
     * 
     */
    budget: ContextPrepareInputBudget;
    /**
     * 
     */
    observed: TimePointInput;
}
/**
 * 
 * @export
 * @interface ContextPrepareInputBudget
 */
export interface ContextPrepareInputBudget {
    /**
     * 
     */
    outputTokens: number;
    /**
     * 
     */
    toolTokens?: number;
    /**
     * 
     */
    requiredTokens?: number;
    /**
     * 
     */
    mediaTokens?: number;
    /**
     * 
     */
    cognitionTokens?: number;
}
/**
 * 
 * @export
 * @interface ContextPrepareInputModel
 */
export interface ContextPrepareInputModel {
    /**
     * 
     */
    resourceId: string;
    /**
     * 
     */
    modelId: string;
    /**
     * 
     */
    contextLimit: number;
}
/**
 * 
 * @export
 * @interface ContextPrepareModel
 */
export interface ContextPrepareModel {
    /**
     * 
     */
    resourceId: string;
    /**
     * 
     */
    modelId: string;
    /**
     * 
     */
    contextLimit: number;
}
/**
 * 
 * @export
 * @interface ContextView
 */
export interface ContextView {
    /**
     * 
     */
    id: string;
    /**
     * 
     */
    ref: Ref;
    /**
     * 
     */
    subjectId: string;
    /**
     * 
     */
    consumer: Consumer;
    /**
     * 
     */
    workContextId?: string;
    /**
     * 
     */
    role: string;
    /**
     * 
     */
    blocks: Array<ContextBlock>;
    /**
     * 
     */
    selectedRefs: Array<Ref>;
    /**
     * 
     */
    inputPortions: Array<InputPortion>;
    /**
     * 
     */
    trimmed: Array<ContextViewTrimmedInner>;
    /**
     * 
     */
    model: ContextPrepareModel;
    /**
     * 
     */
    budget: ContextViewBudget;
    /**
     * 
     */
    effectivePolicy: ContextViewEffectivePolicy;
    /**
     * 
     */
    settingsRevisions: { [key: string]: number; };
    /**
     * 
     */
    observed: TimePoint;
    /**
     * 
     */
    recorded: string;
}
/**
 * 
 * @export
 * @interface ContextViewBudget
 */
export interface ContextViewBudget {
    /**
     * 
     */
    availableTokens: number;
    /**
     * 
     */
    usedTokens: number;
    /**
     * 
     */
    reservedTokens: number;
    /**
     * 
     */
    estimate: ContextViewBudgetEstimateEnum;
}


/**
 * @export
 */
export const ContextViewBudgetEstimateEnum = {
    UnicodeCodePoints: 'unicode_code_points',
} as const;
export type ContextViewBudgetEstimateEnum = typeof ContextViewBudgetEstimateEnum[keyof typeof ContextViewBudgetEstimateEnum];

/**
 * 
 * @export
 * @interface ContextViewEffectivePolicy
 */
export interface ContextViewEffectivePolicy {
    /**
     * 
     */
    purpose: string;
    /**
     * 
     */
    audience: Array<string>;
}
/**
 * 
 * @export
 * @interface ContextViewInput
 */
export interface ContextViewInput {
    /**
     * 
     */
    id: string;
    /**
     * 
     */
    ref: RefInput;
    /**
     * 
     */
    subjectId: string;
    /**
     * 
     */
    consumer: ConsumerInput;
    /**
     * 
     */
    workContextId?: string;
    /**
     * 
     */
    role: string;
    /**
     * 
     */
    blocks: Array<ContextBlockInput>;
    /**
     * 
     */
    selectedRefs: Array<RefInput>;
    /**
     * 
     */
    inputPortions: Array<InputPortionInput>;
    /**
     * 
     */
    trimmed: Array<ContextViewInputTrimmedInner>;
    /**
     * 
     */
    model: ContextPrepareInputModel;
    /**
     * 
     */
    budget: ContextViewInputBudget;
    /**
     * 
     */
    effectivePolicy: ContextViewInputEffectivePolicy;
    /**
     * 
     */
    settingsRevisions: { [key: string]: number; };
    /**
     * 
     */
    observed: TimePointInput;
    /**
     * 
     */
    recorded: string;
}
/**
 * 
 * @export
 * @interface ContextViewInputBudget
 */
export interface ContextViewInputBudget {
    /**
     * 
     */
    availableTokens: number;
    /**
     * 
     */
    usedTokens: number;
    /**
     * 
     */
    reservedTokens: number;
    /**
     * 
     */
    estimate: ContextViewInputBudgetEstimateEnum;
}


/**
 * @export
 */
export const ContextViewInputBudgetEstimateEnum = {
    UnicodeCodePoints: 'unicode_code_points',
} as const;
export type ContextViewInputBudgetEstimateEnum = typeof ContextViewInputBudgetEstimateEnum[keyof typeof ContextViewInputBudgetEstimateEnum];

/**
 * 
 * @export
 * @interface ContextViewInputEffectivePolicy
 */
export interface ContextViewInputEffectivePolicy {
    /**
     * 
     */
    purpose: string;
    /**
     * 
     */
    audience: Array<string>;
}
/**
 * 
 * @export
 * @interface ContextViewInputTrimmedInner
 */
export interface ContextViewInputTrimmedInner {
    /**
     * 
     */
    ref?: RefInput;
    /**
     * 
     */
    reason: string;
    /**
     * 
     */
    originalCodePoints: number;
    /**
     * 
     */
    includedCodePoints: number;
}
/**
 * 
 * @export
 * @interface ContextViewTrimmedInner
 */
export interface ContextViewTrimmedInner {
    /**
     * 
     */
    ref?: Ref;
    /**
     * 
     */
    reason: string;
    /**
     * 
     */
    originalCodePoints: number;
    /**
     * 
     */
    includedCodePoints: number;
}
/**
 * 
 * @export
 * @interface ContextsUpdateWorkRequest
 */
export interface ContextsUpdateWorkRequest {
    /**
     * 
     */
    expectedRevision: number;
    /**
     * 
     */
    body: WorkContextBodyInput;
}
/**
 * 
 * @export
 * @interface CredentialsIssue200Response
 */
export interface CredentialsIssue200Response {
    /**
     * 
     */
    id: string;
    /**
     * 
     */
    key: string;
}
/**
 * 
 * @export
 * @interface CredentialsIssueRequest
 */
export interface CredentialsIssueRequest {
    /**
     * 
     */
    name: string;
    /**
     * 
     */
    actor: ActorInput;
}
/**
 * 
 * @export
 * @interface Criterion
 */
export interface Criterion {
    /**
     * 
     */
    mode: CriterionModeEnum;
    /**
     * 
     */
    range: TimeRange;
    /**
     * 
     */
    condition: string;
    /**
     * 
     */
    predicate?: CriterionPredicate;
    /**
     * 
     */
    requiredCoverage: number;
}


/**
 * @export
 */
export const CriterionModeEnum = {
    Achieve: 'achieve',
    Terminate: 'terminate',
    Maintain: 'maintain',
    Avoid: 'avoid',
} as const;
export type CriterionModeEnum = typeof CriterionModeEnum[keyof typeof CriterionModeEnum];

/**
 * 
 * @export
 * @interface CriterionInput
 */
export interface CriterionInput {
    /**
     * 
     */
    mode: CriterionInputModeEnum;
    /**
     * 
     */
    range: TimeRangeInput;
    /**
     * 
     */
    condition: string;
    /**
     * 
     */
    predicate?: CriterionInputPredicate;
    /**
     * 
     */
    requiredCoverage?: number;
}


/**
 * @export
 */
export const CriterionInputModeEnum = {
    Achieve: 'achieve',
    Terminate: 'terminate',
    Maintain: 'maintain',
    Avoid: 'avoid',
} as const;
export type CriterionInputModeEnum = typeof CriterionInputModeEnum[keyof typeof CriterionInputModeEnum];

/**
 * 
 * @export
 * @interface CriterionInputPredicate
 */
export interface CriterionInputPredicate {
    /**
     * 
     */
    path: Array<string>;
    /**
     * 
     */
    operator: CriterionInputPredicateOperatorEnum;
    /**
     * 
     */
    value?: CriterionInputPredicateValue | null;
}


/**
 * @export
 */
export const CriterionInputPredicateOperatorEnum = {
    Eq: 'eq',
    Neq: 'neq',
    Gt: 'gt',
    Gte: 'gte',
    Lt: 'lt',
    Lte: 'lte',
    Contains: 'contains',
    Exists: 'exists',
} as const;
export type CriterionInputPredicateOperatorEnum = typeof CriterionInputPredicateOperatorEnum[keyof typeof CriterionInputPredicateOperatorEnum];

/**
 * 
 * @export
 * @interface CriterionInputPredicateValue
 */
export interface CriterionInputPredicateValue {
}
/**
 * 
 * @export
 * @interface CriterionPredicate
 */
export interface CriterionPredicate {
    /**
     * 
     */
    path: Array<string>;
    /**
     * 
     */
    operator: CriterionPredicateOperatorEnum;
    /**
     * 
     */
    value?: CriterionInputPredicateValue | null;
}


/**
 * @export
 */
export const CriterionPredicateOperatorEnum = {
    Eq: 'eq',
    Neq: 'neq',
    Gt: 'gt',
    Gte: 'gte',
    Lt: 'lt',
    Lte: 'lte',
    Contains: 'contains',
    Exists: 'exists',
} as const;
export type CriterionPredicateOperatorEnum = typeof CriterionPredicateOperatorEnum[keyof typeof CriterionPredicateOperatorEnum];

/**
 * 
 * @export
 * @interface EncounterAccept
 */
export interface EncounterAccept {
    /**
     * 
     */
    consumer: Consumer;
    /**
     * 
     */
    invocationId: string;
    /**
     * 
     */
    step: EncounterAcceptInputStep;
    /**
     * 
     */
    contextViewRef?: Ref;
    /**
     * 
     */
    portions: Array<Portion>;
    /**
     * 
     */
    resultRef?: Ref;
    /**
     * 
     */
    observed: TimePoint;
    /**
     * 
     */
    occurred?: TimeRange;
    /**
     * 
     */
    activityRef?: Ref;
    /**
     * 
     */
    contextRef?: Ref;
    /**
     * 
     */
    purpose: string;
    /**
     * 
     */
    completion: EncounterAcceptCompletionEnum;
}


/**
 * @export
 */
export const EncounterAcceptCompletionEnum = {
    Complete: 'complete',
    UsablePartial: 'usable_partial',
} as const;
export type EncounterAcceptCompletionEnum = typeof EncounterAcceptCompletionEnum[keyof typeof EncounterAcceptCompletionEnum];

/**
 * 
 * @export
 * @interface EncounterAcceptInput
 */
export interface EncounterAcceptInput {
    /**
     * 
     */
    consumer: ConsumerInput;
    /**
     * 
     */
    invocationId: string;
    /**
     * 
     */
    step: EncounterAcceptInputStep;
    /**
     * 
     */
    contextViewRef?: RefInput;
    /**
     * 
     */
    portions: Array<PortionInput>;
    /**
     * 
     */
    resultRef?: RefInput;
    /**
     * 
     */
    observed: TimePointInput;
    /**
     * 
     */
    occurred?: TimeRangeInput;
    /**
     * 
     */
    activityRef?: RefInput;
    /**
     * 
     */
    contextRef?: RefInput;
    /**
     * 
     */
    purpose: string;
    /**
     * 
     */
    completion?: EncounterAcceptInputCompletionEnum;
}


/**
 * @export
 */
export const EncounterAcceptInputCompletionEnum = {
    Complete: 'complete',
    UsablePartial: 'usable_partial',
} as const;
export type EncounterAcceptInputCompletionEnum = typeof EncounterAcceptInputCompletionEnum[keyof typeof EncounterAcceptInputCompletionEnum];

/**
 * 
 * @export
 * @interface EncounterAcceptInputStep
 */
export interface EncounterAcceptInputStep {
}
/**
 * 
 * @export
 * @interface EncounterView
 */
export interface EncounterView {
    /**
     * 
     */
    consumer: Consumer;
    /**
     * 
     */
    invocationId: string;
    /**
     * 
     */
    step: EncounterAcceptInputStep;
    /**
     * 
     */
    contextViewRef?: Ref;
    /**
     * 
     */
    portions: Array<Portion>;
    /**
     * 
     */
    resultRef?: Ref;
    /**
     * 
     */
    observed: TimePoint;
    /**
     * 
     */
    occurred?: TimeRange;
    /**
     * 
     */
    activityRef?: Ref;
    /**
     * 
     */
    contextRef?: Ref;
    /**
     * 
     */
    purpose: string;
    /**
     * 
     */
    completion: EncounterViewCompletionEnum;
    /**
     * 
     */
    ref: Ref;
    /**
     * 
     */
    recordedAt: string;
}


/**
 * @export
 */
export const EncounterViewCompletionEnum = {
    Complete: 'complete',
    UsablePartial: 'usable_partial',
} as const;
export type EncounterViewCompletionEnum = typeof EncounterViewCompletionEnum[keyof typeof EncounterViewCompletionEnum];

/**
 * 
 * @export
 * @interface EncounterViewInput
 */
export interface EncounterViewInput {
    /**
     * 
     */
    consumer: ConsumerInput;
    /**
     * 
     */
    invocationId: string;
    /**
     * 
     */
    step: EncounterAcceptInputStep;
    /**
     * 
     */
    contextViewRef?: RefInput;
    /**
     * 
     */
    portions: Array<PortionInput>;
    /**
     * 
     */
    resultRef?: RefInput;
    /**
     * 
     */
    observed: TimePointInput;
    /**
     * 
     */
    occurred?: TimeRangeInput;
    /**
     * 
     */
    activityRef?: RefInput;
    /**
     * 
     */
    contextRef?: RefInput;
    /**
     * 
     */
    purpose: string;
    /**
     * 
     */
    completion?: EncounterViewInputCompletionEnum;
    /**
     * 
     */
    ref: RefInput;
    /**
     * 
     */
    recordedAt: string;
}


/**
 * @export
 */
export const EncounterViewInputCompletionEnum = {
    Complete: 'complete',
    UsablePartial: 'usable_partial',
} as const;
export type EncounterViewInputCompletionEnum = typeof EncounterViewInputCompletionEnum[keyof typeof EncounterViewInputCompletionEnum];

/**
 * 
 * @export
 * @interface Episode
 */
export interface Episode {
    /**
     * 
     */
    content: string;
    /**
     * 
     */
    title?: string;
    /**
     * 
     */
    entityRefs: Array<Ref>;
    /**
     * 
     */
    conceptRefs: Array<Ref>;
    /**
     * 
     */
    tags: Array<string>;
    /**
     * 
     */
    kind: EpisodeKindEnum;
    /**
     * 
     */
    track: EpisodeTrackEnum;
    /**
     * 
     */
    range: TimeRange;
    /**
     * 
     */
    members: Array<Ref>;
    /**
     * 
     */
    participants: Array<Ref>;
    /**
     * 
     */
    placeRef?: Ref;
    /**
     * 
     */
    parentRef?: Ref;
}


/**
 * @export
 */
export const EpisodeKindEnum = {
    Episode: 'Episode',
} as const;
export type EpisodeKindEnum = typeof EpisodeKindEnum[keyof typeof EpisodeKindEnum];

/**
 * @export
 */
export const EpisodeTrackEnum = {
    Life: 'life',
    Social: 'social',
    Work: 'work',
} as const;
export type EpisodeTrackEnum = typeof EpisodeTrackEnum[keyof typeof EpisodeTrackEnum];

/**
 * 
 * @export
 * @interface EpisodeInput
 */
export interface EpisodeInput {
    /**
     * 
     */
    content: string;
    /**
     * 
     */
    title?: string;
    /**
     * 
     */
    entityRefs?: Array<RefInput>;
    /**
     * 
     */
    conceptRefs?: Array<RefInput>;
    /**
     * 
     */
    tags?: Array<string>;
    /**
     * 
     */
    kind: EpisodeInputKindEnum;
    /**
     * 
     */
    track: EpisodeInputTrackEnum;
    /**
     * 
     */
    range: TimeRangeInput;
    /**
     * 
     */
    members: Array<RefInput>;
    /**
     * 
     */
    participants?: Array<RefInput>;
    /**
     * 
     */
    placeRef?: RefInput;
    /**
     * 
     */
    parentRef?: RefInput;
}


/**
 * @export
 */
export const EpisodeInputKindEnum = {
    Episode: 'Episode',
} as const;
export type EpisodeInputKindEnum = typeof EpisodeInputKindEnum[keyof typeof EpisodeInputKindEnum];

/**
 * @export
 */
export const EpisodeInputTrackEnum = {
    Life: 'life',
    Social: 'social',
    Work: 'work',
} as const;
export type EpisodeInputTrackEnum = typeof EpisodeInputTrackEnum[keyof typeof EpisodeInputTrackEnum];

/**
 * 
 * @export
 * @interface EpisodesOrganizeRequest
 */
export interface EpisodesOrganizeRequest {
    /**
     * 
     */
    track: EpisodesOrganizeRequestTrackEnum;
    /**
     * 
     */
    experienceRefs: Array<RefInput>;
    /**
     * 
     */
    formed: TimePointInput;
    /**
     * 
     */
    reason: string;
    /**
     * 
     */
    episodes: Array<EpisodesOrganizeRequestEpisodesInner>;
    /**
     * 
     */
    replaced?: Array<EpisodesOrganizeRequestEpisodesInnerTarget>;
}


/**
 * @export
 */
export const EpisodesOrganizeRequestTrackEnum = {
    Life: 'life',
    Social: 'social',
    Work: 'work',
} as const;
export type EpisodesOrganizeRequestTrackEnum = typeof EpisodesOrganizeRequestTrackEnum[keyof typeof EpisodesOrganizeRequestTrackEnum];

/**
 * 
 * @export
 * @interface EpisodesOrganizeRequestEpisodesInner
 */
export interface EpisodesOrganizeRequestEpisodesInner {
    /**
     * 
     */
    target?: EpisodesOrganizeRequestEpisodesInnerTarget;
    /**
     * 
     */
    title: string;
    /**
     * 
     */
    summary: string;
    /**
     * 
     */
    range: TimeRangeInput;
    /**
     * 
     */
    memberRefs: Array<RefInput>;
    /**
     * 
     */
    participantRefs?: Array<RefInput>;
    /**
     * 
     */
    placeRef?: RefInput;
    /**
     * 
     */
    parentRef?: RefInput;
}
/**
 * 
 * @export
 * @interface EpisodesOrganizeRequestEpisodesInnerTarget
 */
export interface EpisodesOrganizeRequestEpisodesInnerTarget {
    /**
     * 
     */
    id: string;
    /**
     * 
     */
    expectedRevision: number;
}
/**
 * 
 * @export
 * @interface Expand
 */
export interface Expand {
    /**
     * 
     */
    refs: Array<Ref>;
    /**
     * 
     */
    include: Array<ExpandIncludeEnum>;
    /**
     * 
     */
    depth: number;
    /**
     * 
     */
    limit: number;
}


/**
 * @export
 */
export const ExpandIncludeEnum = {
    Sources: 'sources',
    Relations: 'relations',
} as const;
export type ExpandIncludeEnum = typeof ExpandIncludeEnum[keyof typeof ExpandIncludeEnum];

/**
 * 
 * @export
 * @interface ExpandInput
 */
export interface ExpandInput {
    /**
     * 
     */
    refs: Array<RefInput>;
    /**
     * 
     */
    include?: Array<ExpandInputIncludeEnum>;
    /**
     * 
     */
    depth?: number;
    /**
     * 
     */
    limit?: number;
}


/**
 * @export
 */
export const ExpandInputIncludeEnum = {
    Sources: 'sources',
    Relations: 'relations',
} as const;
export type ExpandInputIncludeEnum = typeof ExpandInputIncludeEnum[keyof typeof ExpandInputIncludeEnum];

/**
 * 
 * @export
 * @interface ExpansionResult
 */
export interface ExpansionResult {
    /**
     * 
     */
    items: Array<QueryItem>;
    /**
     * 
     */
    sources: Array<MaterialView>;
    /**
     * 
     */
    relations: Array<ExpansionResultRelationsInner>;
}
/**
 * 
 * @export
 * @interface ExpansionResultInput
 */
export interface ExpansionResultInput {
    /**
     * 
     */
    items: Array<QueryItemInput>;
    /**
     * 
     */
    sources: Array<MaterialViewInput>;
    /**
     * 
     */
    relations: Array<ExpansionResultInputRelationsInner>;
}
/**
 * 
 * @export
 * @interface ExpansionResultInputRelationsInner
 */
export interface ExpansionResultInputRelationsInner {
    /**
     * 
     */
    from: RefInput;
    /**
     * 
     */
    to: RefInput;
    /**
     * 
     */
    relation: string;
    /**
     * 
     */
    weight: number;
}
/**
 * 
 * @export
 * @interface ExpansionResultRelationsInner
 */
export interface ExpansionResultRelationsInner {
    /**
     * 
     */
    from: Ref;
    /**
     * 
     */
    to: Ref;
    /**
     * 
     */
    relation: string;
    /**
     * 
     */
    weight: number;
}
/**
 * 
 * @export
 * @interface ExperienceItem
 */
export interface ExperienceItem {
    /**
     * 
     */
    ref: Ref;
    /**
     * 
     */
    encounterRef: Ref;
    /**
     * 
     */
    observed: TimePoint;
    /**
     * 
     */
    portions: Array<Portion>;
    /**
     * 
     */
    activityRef?: Ref;
    /**
     * 
     */
    contextRef?: Ref;
    /**
     * 
     */
    sourceRoots: Array<Ref>;
}
/**
 * 
 * @export
 * @interface ExperienceItemInput
 */
export interface ExperienceItemInput {
    /**
     * 
     */
    ref: RefInput;
    /**
     * 
     */
    encounterRef: RefInput;
    /**
     * 
     */
    observed: TimePointInput;
    /**
     * 
     */
    portions: Array<PortionInput>;
    /**
     * 
     */
    activityRef?: RefInput;
    /**
     * 
     */
    contextRef?: RefInput;
    /**
     * 
     */
    sourceRoots: Array<RefInput>;
}
/**
 * 
 * @export
 * @interface Feedback
 */
export interface Feedback {
    /**
     * 
     */
    stepKey: string;
    /**
     * 
     */
    kind: FeedbackKindEnum;
    /**
     * 
     */
    refs: Array<Ref>;
    /**
     * 
     */
    basisRefs: Array<Ref>;
}


/**
 * @export
 */
export const FeedbackKindEnum = {
    Presented: 'presented',
    Cited: 'cited',
    Adopted: 'adopted',
    Outcome: 'outcome',
    Corrected: 'corrected',
} as const;
export type FeedbackKindEnum = typeof FeedbackKindEnum[keyof typeof FeedbackKindEnum];

/**
 * 
 * @export
 * @interface FeedbackInput
 */
export interface FeedbackInput {
    /**
     * 
     */
    stepKey: string;
    /**
     * 
     */
    kind: FeedbackInputKindEnum;
    /**
     * 
     */
    refs: Array<RefInput>;
    /**
     * 
     */
    basisRefs?: Array<RefInput>;
}


/**
 * @export
 */
export const FeedbackInputKindEnum = {
    Presented: 'presented',
    Cited: 'cited',
    Adopted: 'adopted',
    Outcome: 'outcome',
    Corrected: 'corrected',
} as const;
export type FeedbackInputKindEnum = typeof FeedbackInputKindEnum[keyof typeof FeedbackInputKindEnum];

/**
 * 
 * @export
 * @interface FormationArtifact
 */
export interface FormationArtifact {
    /**
     * 
     */
    id: string;
    /**
     * 
     */
    ref: Ref;
    /**
     * 
     */
    subjectId: string;
    /**
     * 
     */
    purpose: FormationArtifactPurposeEnum;
    /**
     * 
     */
    previousArtifactRef?: Ref;
    /**
     * 
     */
    appliedArtifactRef?: Ref;
    /**
     * 
     */
    experienceRefs: Array<Ref>;
    /**
     * 
     */
    sourceRefs: Array<Ref>;
    /**
     * 
     */
    changeSet: ChangeSet;
    /**
     * 
     */
    groups: Array<FormationArtifactGroupsInner>;
    /**
     * 
     */
    modelInvocationId?: string;
    /**
     * 
     */
    resolutions: Array<FormationArtifactResolutionsInner>;
    /**
     * 
     */
    modelOutputRef?: Ref;
    /**
     * 
     */
    recorded: string;
}


/**
 * @export
 */
export const FormationArtifactPurposeEnum = {
    Growth: 'growth',
    Seed: 'seed',
} as const;
export type FormationArtifactPurposeEnum = typeof FormationArtifactPurposeEnum[keyof typeof FormationArtifactPurposeEnum];

/**
 * 
 * @export
 * @interface FormationArtifactGroupsInner
 */
export interface FormationArtifactGroupsInner {
    /**
     * 
     */
    id: string;
    /**
     * 
     */
    status: FormationArtifactGroupsInnerStatusEnum;
    /**
     * 
     */
    issues: Array<string>;
    /**
     * 
     */
    createdRefs: Array<Ref>;
    /**
     * 
     */
    revisedRefs: Array<Ref>;
    /**
     * 
     */
    conflicts: Array<FormationArtifactGroupsInnerConflictsInner>;
}


/**
 * @export
 */
export const FormationArtifactGroupsInnerStatusEnum = {
    Pending: 'pending',
    Applied: 'applied',
    Conflict: 'conflict',
    Invalid: 'invalid',
    Discarded: 'discarded',
} as const;
export type FormationArtifactGroupsInnerStatusEnum = typeof FormationArtifactGroupsInnerStatusEnum[keyof typeof FormationArtifactGroupsInnerStatusEnum];

/**
 * 
 * @export
 * @interface FormationArtifactGroupsInnerConflictsInner
 */
export interface FormationArtifactGroupsInnerConflictsInner {
    /**
     * 
     */
    id: string;
    /**
     * 
     */
    expectedRevision: number;
    /**
     * 
     */
    currentRevision?: number;
}
/**
 * 
 * @export
 * @interface FormationArtifactInput
 */
export interface FormationArtifactInput {
    /**
     * 
     */
    id: string;
    /**
     * 
     */
    ref: RefInput;
    /**
     * 
     */
    subjectId: string;
    /**
     * 
     */
    purpose?: FormationArtifactInputPurposeEnum;
    /**
     * 
     */
    previousArtifactRef?: RefInput;
    /**
     * 
     */
    appliedArtifactRef?: RefInput;
    /**
     * 
     */
    experienceRefs: Array<RefInput>;
    /**
     * 
     */
    sourceRefs: Array<RefInput>;
    /**
     * 
     */
    changeSet: ChangeSetInput;
    /**
     * 
     */
    groups: Array<FormationArtifactInputGroupsInner>;
    /**
     * 
     */
    modelInvocationId?: string;
    /**
     * 
     */
    resolutions?: Array<FormationArtifactInputResolutionsInner>;
    /**
     * 
     */
    modelOutputRef?: RefInput;
    /**
     * 
     */
    recorded: string;
}


/**
 * @export
 */
export const FormationArtifactInputPurposeEnum = {
    Growth: 'growth',
    Seed: 'seed',
} as const;
export type FormationArtifactInputPurposeEnum = typeof FormationArtifactInputPurposeEnum[keyof typeof FormationArtifactInputPurposeEnum];

/**
 * 
 * @export
 * @interface FormationArtifactInputGroupsInner
 */
export interface FormationArtifactInputGroupsInner {
    /**
     * 
     */
    id: string;
    /**
     * 
     */
    status: FormationArtifactInputGroupsInnerStatusEnum;
    /**
     * 
     */
    issues?: Array<string>;
    /**
     * 
     */
    createdRefs: Array<RefInput>;
    /**
     * 
     */
    revisedRefs: Array<RefInput>;
    /**
     * 
     */
    conflicts: Array<FormationArtifactInputGroupsInnerConflictsInner>;
}


/**
 * @export
 */
export const FormationArtifactInputGroupsInnerStatusEnum = {
    Pending: 'pending',
    Applied: 'applied',
    Conflict: 'conflict',
    Invalid: 'invalid',
    Discarded: 'discarded',
} as const;
export type FormationArtifactInputGroupsInnerStatusEnum = typeof FormationArtifactInputGroupsInnerStatusEnum[keyof typeof FormationArtifactInputGroupsInnerStatusEnum];

/**
 * 
 * @export
 * @interface FormationArtifactInputGroupsInnerConflictsInner
 */
export interface FormationArtifactInputGroupsInnerConflictsInner {
    /**
     * 
     */
    id: string;
    /**
     * 
     */
    expectedRevision: number;
    /**
     * 
     */
    currentRevision?: number;
}
/**
 * 
 * @export
 * @interface FormationArtifactInputResolutionsInner
 */
export interface FormationArtifactInputResolutionsInner {
    /**
     * 
     */
    groupIds: Array<string>;
    /**
     * 
     */
    previous: ChangeSetInput;
    /**
     * 
     */
    action: FormationArtifactInputResolutionsInnerActionEnum;
    /**
     * 
     */
    reason: string;
    /**
     * 
     */
    at: TimePointInput;
    /**
     * 
     */
    modelInvocationId?: string;
}


/**
 * @export
 */
export const FormationArtifactInputResolutionsInnerActionEnum = {
    Edit: 'edit',
    Rejudge: 'rejudge',
    Discard: 'discard',
} as const;
export type FormationArtifactInputResolutionsInnerActionEnum = typeof FormationArtifactInputResolutionsInnerActionEnum[keyof typeof FormationArtifactInputResolutionsInnerActionEnum];

/**
 * 
 * @export
 * @interface FormationArtifactResolutionsInner
 */
export interface FormationArtifactResolutionsInner {
    /**
     * 
     */
    groupIds: Array<string>;
    /**
     * 
     */
    previous: ChangeSet;
    /**
     * 
     */
    action: FormationArtifactResolutionsInnerActionEnum;
    /**
     * 
     */
    reason: string;
    /**
     * 
     */
    at: TimePoint;
    /**
     * 
     */
    modelInvocationId?: string;
}


/**
 * @export
 */
export const FormationArtifactResolutionsInnerActionEnum = {
    Edit: 'edit',
    Rejudge: 'rejudge',
    Discard: 'discard',
} as const;
export type FormationArtifactResolutionsInnerActionEnum = typeof FormationArtifactResolutionsInnerActionEnum[keyof typeof FormationArtifactResolutionsInnerActionEnum];

/**
 * 
 * @export
 * @interface GrowthApplyRequest
 */
export interface GrowthApplyRequest {
    /**
     * 
     */
    groupIds?: Array<string>;
}
/**
 * 
 * @export
 * @interface GrowthResolve
 */
export interface GrowthResolve {
    /**
     * 
     */
    id: string;
    /**
     * 
     */
    groupIds: Array<string>;
    /**
     * 
     */
    action: GrowthResolveActionEnum;
    /**
     * 
     */
    groups?: Array<ChangeSetGroupsInner>;
    /**
     * 
     */
    formed: TimePoint;
    /**
     * 
     */
    reason: string;
}


/**
 * @export
 */
export const GrowthResolveActionEnum = {
    Edit: 'edit',
    Rejudge: 'rejudge',
    Discard: 'discard',
} as const;
export type GrowthResolveActionEnum = typeof GrowthResolveActionEnum[keyof typeof GrowthResolveActionEnum];

/**
 * 
 * @export
 * @interface GrowthResolveInput
 */
export interface GrowthResolveInput {
    /**
     * 
     */
    id: string;
    /**
     * 
     */
    groupIds: Array<string>;
    /**
     * 
     */
    action: GrowthResolveInputActionEnum;
    /**
     * 
     */
    groups?: Array<GrowthResolveRequestGroupsInner>;
    /**
     * 
     */
    formed: TimePointInput;
    /**
     * 
     */
    reason: string;
}


/**
 * @export
 */
export const GrowthResolveInputActionEnum = {
    Edit: 'edit',
    Rejudge: 'rejudge',
    Discard: 'discard',
} as const;
export type GrowthResolveInputActionEnum = typeof GrowthResolveInputActionEnum[keyof typeof GrowthResolveInputActionEnum];

/**
 * 
 * @export
 * @interface GrowthResolveRequest
 */
export interface GrowthResolveRequest {
    /**
     * 
     */
    groupIds: Array<string>;
    /**
     * 
     */
    action: GrowthResolveRequestActionEnum;
    /**
     * 
     */
    groups?: Array<GrowthResolveRequestGroupsInner>;
    /**
     * 
     */
    formed: TimePointInput;
    /**
     * 
     */
    reason: string;
}


/**
 * @export
 */
export const GrowthResolveRequestActionEnum = {
    Edit: 'edit',
    Rejudge: 'rejudge',
    Discard: 'discard',
} as const;
export type GrowthResolveRequestActionEnum = typeof GrowthResolveRequestActionEnum[keyof typeof GrowthResolveRequestActionEnum];

/**
 * 
 * @export
 * @interface GrowthResolveRequestGroupsInner
 */
export interface GrowthResolveRequestGroupsInner {
    /**
     * 
     */
    id: string;
    /**
     * 
     */
    reason: string;
    /**
     * 
     */
    changes: Array<KnowledgeChangeInput>;
}
/**
 * 
 * @export
 * @interface GrowthRunRequest
 */
export interface GrowthRunRequest {
    /**
     * 
     */
    experienceRefs?: Array<RefInput>;
    /**
     * 
     */
    formed?: TimePointInput;
    /**
     * 
     */
    apply?: boolean;
}
/**
 * 
 * @export
 * @interface InputPortion
 */
export interface InputPortion {
    /**
     * 
     */
    ref: Ref;
    /**
     * 
     */
    text: string;
    /**
     * 
     */
    selector?: Selector;
    /**
     * 
     */
    required: boolean;
}
/**
 * 
 * @export
 * @interface InputPortionInput
 */
export interface InputPortionInput {
    /**
     * 
     */
    ref: RefInput;
    /**
     * 
     */
    text: string;
    /**
     * 
     */
    selector?: SelectorInput;
    /**
     * 
     */
    required?: boolean;
}
/**
 * 
 * @export
 * @interface InvocationCompletion
 */
export interface InvocationCompletion {
    /**
     * 
     */
    status: InvocationCompletionStatusEnum;
    /**
     * 
     */
    inputTokens?: number;
    /**
     * 
     */
    outputTokens?: number;
    /**
     * 
     */
    cacheReadTokens?: number;
    /**
     * 
     */
    cacheWriteTokens?: number;
    /**
     * 
     */
    providerRequestId?: string;
    /**
     * 
     */
    outputRef?: Ref;
    /**
     * 
     */
    rawUsage?: { [key: string]: any; };
}


/**
 * @export
 */
export const InvocationCompletionStatusEnum = {
    Completed: 'completed',
    NotSent: 'not_sent',
    Unknown: 'unknown',
} as const;
export type InvocationCompletionStatusEnum = typeof InvocationCompletionStatusEnum[keyof typeof InvocationCompletionStatusEnum];

/**
 * 
 * @export
 * @interface InvocationCompletionInput
 */
export interface InvocationCompletionInput {
    /**
     * 
     */
    status: InvocationCompletionInputStatusEnum;
    /**
     * 
     */
    inputTokens?: number;
    /**
     * 
     */
    outputTokens?: number;
    /**
     * 
     */
    cacheReadTokens?: number;
    /**
     * 
     */
    cacheWriteTokens?: number;
    /**
     * 
     */
    providerRequestId?: string;
    /**
     * 
     */
    outputRef?: RefInput;
    /**
     * 
     */
    rawUsage?: { [key: string]: any; };
}


/**
 * @export
 */
export const InvocationCompletionInputStatusEnum = {
    Completed: 'completed',
    NotSent: 'not_sent',
    Unknown: 'unknown',
} as const;
export type InvocationCompletionInputStatusEnum = typeof InvocationCompletionInputStatusEnum[keyof typeof InvocationCompletionInputStatusEnum];

/**
 * 
 * @export
 * @interface InvocationPrepare
 */
export interface InvocationPrepare {
    /**
     * 
     */
    invocationId: string;
    /**
     * 
     */
    role: string;
    /**
     * 
     */
    modelResourceId: string;
    /**
     * 
     */
    inputTokensEstimate: number;
    /**
     * 
     */
    maxOutputTokens: number;
    /**
     * 
     */
    inputRefs: Array<Ref>;
    /**
     * 
     */
    nativeRunRef?: Ref;
}
/**
 * 
 * @export
 * @interface InvocationPrepareInput
 */
export interface InvocationPrepareInput {
    /**
     * 
     */
    invocationId: string;
    /**
     * 
     */
    role: string;
    /**
     * 
     */
    modelResourceId: string;
    /**
     * 
     */
    inputTokensEstimate: number;
    /**
     * 
     */
    maxOutputTokens: number;
    /**
     * 
     */
    inputRefs?: Array<RefInput>;
    /**
     * 
     */
    nativeRunRef?: RefInput;
}
/**
 * 
 * @export
 * @interface Journal
 */
export interface Journal {
    /**
     * 
     */
    content: string;
    /**
     * 
     */
    title?: string;
    /**
     * 
     */
    entityRefs: Array<Ref>;
    /**
     * 
     */
    conceptRefs: Array<Ref>;
    /**
     * 
     */
    tags: Array<string>;
    /**
     * 
     */
    kind: JournalKindEnum;
    /**
     * 
     */
    range: TimeRange;
    /**
     * 
     */
    experienceRefs: Array<Ref>;
    /**
     * 
     */
    perspectiveAt: TimePoint;
    /**
     * 
     */
    mood?: string;
    /**
     * 
     */
    outlook?: string;
}


/**
 * @export
 */
export const JournalKindEnum = {
    Journal: 'Journal',
} as const;
export type JournalKindEnum = typeof JournalKindEnum[keyof typeof JournalKindEnum];

/**
 * 
 * @export
 * @interface JournalInput
 */
export interface JournalInput {
    /**
     * 
     */
    content: string;
    /**
     * 
     */
    title?: string;
    /**
     * 
     */
    entityRefs?: Array<RefInput>;
    /**
     * 
     */
    conceptRefs?: Array<RefInput>;
    /**
     * 
     */
    tags?: Array<string>;
    /**
     * 
     */
    kind: JournalInputKindEnum;
    /**
     * 
     */
    range: TimeRangeInput;
    /**
     * 
     */
    experienceRefs: Array<RefInput>;
    /**
     * 
     */
    perspectiveAt: TimePointInput;
    /**
     * 
     */
    mood?: string;
    /**
     * 
     */
    outlook?: string;
}


/**
 * @export
 */
export const JournalInputKindEnum = {
    Journal: 'Journal',
} as const;
export type JournalInputKindEnum = typeof JournalInputKindEnum[keyof typeof JournalInputKindEnum];

/**
 * 
 * @export
 * @interface JournalsWriteRequest
 */
export interface JournalsWriteRequest {
    /**
     * 
     */
    target?: EpisodesOrganizeRequestEpisodesInnerTarget;
    /**
     * 
     */
    range: TimeRangeInput;
    /**
     * 
     */
    experienceRefs: Array<RefInput>;
    /**
     * 
     */
    knowledgeAt: TimePointInput;
    /**
     * 
     */
    formed: TimePointInput;
    /**
     * 
     */
    content?: string;
    /**
     * 
     */
    title?: string;
    /**
     * 
     */
    mood?: string;
    /**
     * 
     */
    outlook?: string;
    /**
     * 
     */
    reason?: string;
}
/**
 * @type KnowledgeBody
 * 
 * @export
 */
export type KnowledgeBody = CognitiveSchema | Concept | Episode | Journal | LanguageConvention | Memory | Motivation | Self | Social;
/**
 * @type KnowledgeBodyInput
 * 
 * @export
 */
export type KnowledgeBodyInput = CognitiveSchemaInput | ConceptInput | EpisodeInput | JournalInput | LanguageConventionInput | MemoryInput | MotivationInput | SelfInput | SocialInput;
/**
 * 
 * @export
 * @interface KnowledgeChange
 */
export interface KnowledgeChange {
    /**
     * 
     */
    kind: KnowledgeKind;
    /**
     * 
     */
    body: KnowledgeBody;
    /**
     * 
     */
    sourceRefs: Array<Ref>;
    /**
     * 
     */
    formed: TimePoint;
    /**
     * 
     */
    valid?: TimeRange;
    /**
     * 
     */
    formation: KnowledgeChangeFormationEnum;
    /**
     * 
     */
    stance: KnowledgeChangeStanceEnum;
    /**
     * 
     */
    confidence: number;
    /**
     * 
     */
    policy?: Policy;
    /**
     * 
     */
    target?: KnowledgeChangeTarget;
    /**
     * 
     */
    reason: string;
}


/**
 * @export
 */
export const KnowledgeChangeFormationEnum = {
    Observed: 'observed',
    Told: 'told',
    Read: 'read',
    Inferred: 'inferred',
    Reflection: 'reflection',
    InitialSetting: 'initial_setting',
    OperatorEdit: 'operator_edit',
} as const;
export type KnowledgeChangeFormationEnum = typeof KnowledgeChangeFormationEnum[keyof typeof KnowledgeChangeFormationEnum];

/**
 * @export
 */
export const KnowledgeChangeStanceEnum = {
    Accepted: 'accepted',
    Uncertain: 'uncertain',
    Hypothesis: 'hypothesis',
    Disputed: 'disputed',
} as const;
export type KnowledgeChangeStanceEnum = typeof KnowledgeChangeStanceEnum[keyof typeof KnowledgeChangeStanceEnum];

/**
 * 
 * @export
 * @interface KnowledgeChangeInput
 */
export interface KnowledgeChangeInput {
    /**
     * 
     */
    kind: KnowledgeKindInput;
    /**
     * 
     */
    body: KnowledgeBodyInput;
    /**
     * 
     */
    sourceRefs: Array<RefInput>;
    /**
     * 
     */
    formed: TimePointInput;
    /**
     * 
     */
    valid?: TimeRangeInput;
    /**
     * 
     */
    formation: KnowledgeChangeInputFormationEnum;
    /**
     * 
     */
    stance: KnowledgeChangeInputStanceEnum;
    /**
     * 
     */
    confidence?: number;
    /**
     * 
     */
    policy?: PolicyInput;
    /**
     * 
     */
    target?: EpisodesOrganizeRequestEpisodesInnerTarget;
    /**
     * 
     */
    reason: string;
}


/**
 * @export
 */
export const KnowledgeChangeInputFormationEnum = {
    Observed: 'observed',
    Told: 'told',
    Read: 'read',
    Inferred: 'inferred',
    Reflection: 'reflection',
    InitialSetting: 'initial_setting',
    OperatorEdit: 'operator_edit',
} as const;
export type KnowledgeChangeInputFormationEnum = typeof KnowledgeChangeInputFormationEnum[keyof typeof KnowledgeChangeInputFormationEnum];

/**
 * @export
 */
export const KnowledgeChangeInputStanceEnum = {
    Accepted: 'accepted',
    Uncertain: 'uncertain',
    Hypothesis: 'hypothesis',
    Disputed: 'disputed',
} as const;
export type KnowledgeChangeInputStanceEnum = typeof KnowledgeChangeInputStanceEnum[keyof typeof KnowledgeChangeInputStanceEnum];

/**
 * 
 * @export
 * @interface KnowledgeChangeTarget
 */
export interface KnowledgeChangeTarget {
    /**
     * 
     */
    id: string;
    /**
     * 
     */
    expectedRevision: number;
}
/**
 * 
 * @export
 * @interface KnowledgeCreate
 */
export interface KnowledgeCreate {
    /**
     * 
     */
    kind: KnowledgeKind;
    /**
     * 
     */
    body: KnowledgeBody;
    /**
     * 
     */
    sourceRefs: Array<Ref>;
    /**
     * 
     */
    formed: TimePoint;
    /**
     * 
     */
    valid?: TimeRange;
    /**
     * 
     */
    formation: KnowledgeCreateFormationEnum;
    /**
     * 
     */
    stance: KnowledgeCreateStanceEnum;
    /**
     * 
     */
    confidence: number;
    /**
     * 
     */
    policy?: Policy;
}


/**
 * @export
 */
export const KnowledgeCreateFormationEnum = {
    Observed: 'observed',
    Told: 'told',
    Read: 'read',
    Inferred: 'inferred',
    Reflection: 'reflection',
    InitialSetting: 'initial_setting',
    OperatorEdit: 'operator_edit',
} as const;
export type KnowledgeCreateFormationEnum = typeof KnowledgeCreateFormationEnum[keyof typeof KnowledgeCreateFormationEnum];

/**
 * @export
 */
export const KnowledgeCreateStanceEnum = {
    Accepted: 'accepted',
    Uncertain: 'uncertain',
    Hypothesis: 'hypothesis',
    Disputed: 'disputed',
} as const;
export type KnowledgeCreateStanceEnum = typeof KnowledgeCreateStanceEnum[keyof typeof KnowledgeCreateStanceEnum];

/**
 * 
 * @export
 * @interface KnowledgeCreateInput
 */
export interface KnowledgeCreateInput {
    /**
     * 
     */
    kind: KnowledgeKindInput;
    /**
     * 
     */
    body: KnowledgeBodyInput;
    /**
     * 
     */
    sourceRefs: Array<RefInput>;
    /**
     * 
     */
    formed: TimePointInput;
    /**
     * 
     */
    valid?: TimeRangeInput;
    /**
     * 
     */
    formation: KnowledgeCreateInputFormationEnum;
    /**
     * 
     */
    stance: KnowledgeCreateInputStanceEnum;
    /**
     * 
     */
    confidence?: number;
    /**
     * 
     */
    policy?: PolicyInput;
}


/**
 * @export
 */
export const KnowledgeCreateInputFormationEnum = {
    Observed: 'observed',
    Told: 'told',
    Read: 'read',
    Inferred: 'inferred',
    Reflection: 'reflection',
    InitialSetting: 'initial_setting',
    OperatorEdit: 'operator_edit',
} as const;
export type KnowledgeCreateInputFormationEnum = typeof KnowledgeCreateInputFormationEnum[keyof typeof KnowledgeCreateInputFormationEnum];

/**
 * @export
 */
export const KnowledgeCreateInputStanceEnum = {
    Accepted: 'accepted',
    Uncertain: 'uncertain',
    Hypothesis: 'hypothesis',
    Disputed: 'disputed',
} as const;
export type KnowledgeCreateInputStanceEnum = typeof KnowledgeCreateInputStanceEnum[keyof typeof KnowledgeCreateInputStanceEnum];

/**
 * 
 * @export
 * @interface KnowledgeHistory
 */
export interface KnowledgeHistory {
    /**
     * 
     */
    cursor?: string;
    /**
     * 
     */
    limit: number;
    /**
     * 
     */
    knowledgeAt?: TimePoint;
    /**
     * 
     */
    recordedAsOf?: string;
}
/**
 * 
 * @export
 * @interface KnowledgeHistoryInput
 */
export interface KnowledgeHistoryInput {
    /**
     * 
     */
    cursor?: string;
    /**
     * 
     */
    limit?: number;
    /**
     * 
     */
    knowledgeAt?: TimePointInput;
    /**
     * 
     */
    recordedAsOf?: string;
}

/**
 * 
 * @export
 */
export const KnowledgeKind = {
    Memory: 'Memory',
    Episode: 'Episode',
    Journal: 'Journal',
    Schema: 'Schema',
    Concept: 'Concept',
    Self: 'Self',
    Social: 'Social',
    LanguageConvention: 'LanguageConvention',
    Motivation: 'Motivation',
} as const;
export type KnowledgeKind = typeof KnowledgeKind[keyof typeof KnowledgeKind];


/**
 * 
 * @export
 */
export const KnowledgeKindInput = {
    Memory: 'Memory',
    Episode: 'Episode',
    Journal: 'Journal',
    Schema: 'Schema',
    Concept: 'Concept',
    Self: 'Self',
    Social: 'Social',
    LanguageConvention: 'LanguageConvention',
    Motivation: 'Motivation',
} as const;
export type KnowledgeKindInput = typeof KnowledgeKindInput[keyof typeof KnowledgeKindInput];

/**
 * 
 * @export
 * @interface KnowledgeList
 */
export interface KnowledgeList {
    /**
     * 
     */
    kinds?: Array<KnowledgeKind>;
    /**
     * 
     */
    knowledgeAt?: TimePoint;
    /**
     * 
     */
    recordedAsOf?: string;
    /**
     * 
     */
    includeInactive?: boolean;
    /**
     * 
     */
    includeHistory?: boolean;
    /**
     * 
     */
    ids?: Array<string>;
    /**
     * 
     */
    entities?: Array<string>;
    /**
     * 
     */
    entityMatch?: KnowledgeListEntityMatchEnum;
    /**
     * 
     */
    tags?: Array<string>;
    /**
     * 
     */
    tagMatch?: KnowledgeListTagMatchEnum;
    /**
     * 
     */
    refs?: Array<string>;
    /**
     * 
     */
    sources?: Array<string>;
    /**
     * 
     */
    conceptIds?: Array<string>;
    /**
     * 
     */
    cursor?: string;
    /**
     * 
     */
    limit: number;
}


/**
 * @export
 */
export const KnowledgeListEntityMatchEnum = {
    Any: 'any',
    All: 'all',
} as const;
export type KnowledgeListEntityMatchEnum = typeof KnowledgeListEntityMatchEnum[keyof typeof KnowledgeListEntityMatchEnum];

/**
 * @export
 */
export const KnowledgeListTagMatchEnum = {
    Any: 'any',
    All: 'all',
} as const;
export type KnowledgeListTagMatchEnum = typeof KnowledgeListTagMatchEnum[keyof typeof KnowledgeListTagMatchEnum];

/**
 * 
 * @export
 * @interface KnowledgeListInput
 */
export interface KnowledgeListInput {
    /**
     * 
     */
    kinds?: Array<KnowledgeKindInput>;
    /**
     * 
     */
    knowledgeAt?: TimePointInput;
    /**
     * 
     */
    recordedAsOf?: string;
    /**
     * 
     */
    includeInactive?: boolean;
    /**
     * 
     */
    includeHistory?: boolean;
    /**
     * 
     */
    ids?: Array<string>;
    /**
     * 
     */
    entities?: Array<string>;
    /**
     * 
     */
    entityMatch?: KnowledgeListInputEntityMatchEnum;
    /**
     * 
     */
    tags?: Array<string>;
    /**
     * 
     */
    tagMatch?: KnowledgeListInputTagMatchEnum;
    /**
     * 
     */
    refs?: Array<string>;
    /**
     * 
     */
    sources?: Array<string>;
    /**
     * 
     */
    conceptIds?: Array<string>;
    /**
     * 
     */
    cursor?: string;
    /**
     * 
     */
    limit?: number;
}


/**
 * @export
 */
export const KnowledgeListInputEntityMatchEnum = {
    Any: 'any',
    All: 'all',
} as const;
export type KnowledgeListInputEntityMatchEnum = typeof KnowledgeListInputEntityMatchEnum[keyof typeof KnowledgeListInputEntityMatchEnum];

/**
 * @export
 */
export const KnowledgeListInputTagMatchEnum = {
    Any: 'any',
    All: 'all',
} as const;
export type KnowledgeListInputTagMatchEnum = typeof KnowledgeListInputTagMatchEnum[keyof typeof KnowledgeListInputTagMatchEnum];

/**
 * 
 * @export
 * @interface KnowledgePage
 */
export interface KnowledgePage {
    /**
     * 
     */
    items: Array<KnowledgeView>;
    /**
     * 
     */
    nextCursor?: string;
}
/**
 * 
 * @export
 * @interface KnowledgePageInput
 */
export interface KnowledgePageInput {
    /**
     * 
     */
    items: Array<KnowledgeViewInput>;
    /**
     * 
     */
    nextCursor?: string;
}
/**
 * 
 * @export
 * @interface KnowledgeRead
 */
export interface KnowledgeRead {
    /**
     * 
     */
    id: string;
    /**
     * 
     */
    revision?: number;
    /**
     * 
     */
    knowledgeAt?: TimePoint;
    /**
     * 
     */
    recordedAsOf?: string;
}
/**
 * 
 * @export
 * @interface KnowledgeReadInput
 */
export interface KnowledgeReadInput {
    /**
     * 
     */
    id: string;
    /**
     * 
     */
    revision?: number;
    /**
     * 
     */
    knowledgeAt?: TimePointInput;
    /**
     * 
     */
    recordedAsOf?: string;
}
/**
 * 
 * @export
 * @interface KnowledgeRevise
 */
export interface KnowledgeRevise {
    /**
     * 
     */
    kind: KnowledgeKind;
    /**
     * 
     */
    body: KnowledgeBody;
    /**
     * 
     */
    sourceRefs: Array<Ref>;
    /**
     * 
     */
    formed: TimePoint;
    /**
     * 
     */
    valid?: TimeRange;
    /**
     * 
     */
    formation: KnowledgeReviseFormationEnum;
    /**
     * 
     */
    stance: KnowledgeReviseStanceEnum;
    /**
     * 
     */
    confidence: number;
    /**
     * 
     */
    policy?: Policy;
    /**
     * 
     */
    id: string;
    /**
     * 
     */
    expectedRevision: number;
    /**
     * 
     */
    reason: string;
}


/**
 * @export
 */
export const KnowledgeReviseFormationEnum = {
    Observed: 'observed',
    Told: 'told',
    Read: 'read',
    Inferred: 'inferred',
    Reflection: 'reflection',
    InitialSetting: 'initial_setting',
    OperatorEdit: 'operator_edit',
} as const;
export type KnowledgeReviseFormationEnum = typeof KnowledgeReviseFormationEnum[keyof typeof KnowledgeReviseFormationEnum];

/**
 * @export
 */
export const KnowledgeReviseStanceEnum = {
    Accepted: 'accepted',
    Uncertain: 'uncertain',
    Hypothesis: 'hypothesis',
    Disputed: 'disputed',
} as const;
export type KnowledgeReviseStanceEnum = typeof KnowledgeReviseStanceEnum[keyof typeof KnowledgeReviseStanceEnum];

/**
 * 
 * @export
 * @interface KnowledgeReviseInput
 */
export interface KnowledgeReviseInput {
    /**
     * 
     */
    kind: KnowledgeKindInput;
    /**
     * 
     */
    body: KnowledgeBodyInput;
    /**
     * 
     */
    sourceRefs: Array<RefInput>;
    /**
     * 
     */
    formed: TimePointInput;
    /**
     * 
     */
    valid?: TimeRangeInput;
    /**
     * 
     */
    formation: KnowledgeReviseInputFormationEnum;
    /**
     * 
     */
    stance: KnowledgeReviseInputStanceEnum;
    /**
     * 
     */
    confidence?: number;
    /**
     * 
     */
    policy?: PolicyInput;
    /**
     * 
     */
    id: string;
    /**
     * 
     */
    expectedRevision: number;
    /**
     * 
     */
    reason: string;
}


/**
 * @export
 */
export const KnowledgeReviseInputFormationEnum = {
    Observed: 'observed',
    Told: 'told',
    Read: 'read',
    Inferred: 'inferred',
    Reflection: 'reflection',
    InitialSetting: 'initial_setting',
    OperatorEdit: 'operator_edit',
} as const;
export type KnowledgeReviseInputFormationEnum = typeof KnowledgeReviseInputFormationEnum[keyof typeof KnowledgeReviseInputFormationEnum];

/**
 * @export
 */
export const KnowledgeReviseInputStanceEnum = {
    Accepted: 'accepted',
    Uncertain: 'uncertain',
    Hypothesis: 'hypothesis',
    Disputed: 'disputed',
} as const;
export type KnowledgeReviseInputStanceEnum = typeof KnowledgeReviseInputStanceEnum[keyof typeof KnowledgeReviseInputStanceEnum];

/**
 * 
 * @export
 * @interface KnowledgeReviseRequest
 */
export interface KnowledgeReviseRequest {
    /**
     * 
     */
    kind: KnowledgeKindInput;
    /**
     * 
     */
    body: KnowledgeBodyInput;
    /**
     * 
     */
    sourceRefs: Array<RefInput>;
    /**
     * 
     */
    formed: TimePointInput;
    /**
     * 
     */
    valid?: TimeRangeInput;
    /**
     * 
     */
    formation: KnowledgeReviseRequestFormationEnum;
    /**
     * 
     */
    stance: KnowledgeReviseRequestStanceEnum;
    /**
     * 
     */
    confidence?: number;
    /**
     * 
     */
    policy?: PolicyInput;
    /**
     * 
     */
    expectedRevision: number;
    /**
     * 
     */
    reason: string;
}


/**
 * @export
 */
export const KnowledgeReviseRequestFormationEnum = {
    Observed: 'observed',
    Told: 'told',
    Read: 'read',
    Inferred: 'inferred',
    Reflection: 'reflection',
    InitialSetting: 'initial_setting',
    OperatorEdit: 'operator_edit',
} as const;
export type KnowledgeReviseRequestFormationEnum = typeof KnowledgeReviseRequestFormationEnum[keyof typeof KnowledgeReviseRequestFormationEnum];

/**
 * @export
 */
export const KnowledgeReviseRequestStanceEnum = {
    Accepted: 'accepted',
    Uncertain: 'uncertain',
    Hypothesis: 'hypothesis',
    Disputed: 'disputed',
} as const;
export type KnowledgeReviseRequestStanceEnum = typeof KnowledgeReviseRequestStanceEnum[keyof typeof KnowledgeReviseRequestStanceEnum];

/**
 * 
 * @export
 * @interface KnowledgeView
 */
export interface KnowledgeView {
    /**
     * 
     */
    kind: KnowledgeKind;
    /**
     * 
     */
    body: KnowledgeBody;
    /**
     * 
     */
    sourceRefs: Array<Ref>;
    /**
     * 
     */
    formed: TimePoint;
    /**
     * 
     */
    valid?: TimeRange;
    /**
     * 
     */
    formation: KnowledgeViewFormationEnum;
    /**
     * 
     */
    stance: KnowledgeViewStanceEnum;
    /**
     * 
     */
    confidence: number;
    /**
     * 
     */
    policy: Policy;
    /**
     * 
     */
    ref: Ref;
    /**
     * 
     */
    alias: string;
    /**
     * 
     */
    revision: number;
    /**
     * 
     */
    sourceRoots: Array<Ref>;
    /**
     * 
     */
    recorded: TimePoint;
    /**
     * 
     */
    status: KnowledgeViewStatusEnum;
}


/**
 * @export
 */
export const KnowledgeViewFormationEnum = {
    Observed: 'observed',
    Told: 'told',
    Read: 'read',
    Inferred: 'inferred',
    Reflection: 'reflection',
    InitialSetting: 'initial_setting',
    OperatorEdit: 'operator_edit',
} as const;
export type KnowledgeViewFormationEnum = typeof KnowledgeViewFormationEnum[keyof typeof KnowledgeViewFormationEnum];

/**
 * @export
 */
export const KnowledgeViewStanceEnum = {
    Accepted: 'accepted',
    Uncertain: 'uncertain',
    Hypothesis: 'hypothesis',
    Disputed: 'disputed',
} as const;
export type KnowledgeViewStanceEnum = typeof KnowledgeViewStanceEnum[keyof typeof KnowledgeViewStanceEnum];

/**
 * @export
 */
export const KnowledgeViewStatusEnum = {
    Active: 'active',
    Suppressed: 'suppressed',
    Retired: 'retired',
    Purged: 'purged',
} as const;
export type KnowledgeViewStatusEnum = typeof KnowledgeViewStatusEnum[keyof typeof KnowledgeViewStatusEnum];

/**
 * 
 * @export
 * @interface KnowledgeViewInput
 */
export interface KnowledgeViewInput {
    /**
     * 
     */
    kind: KnowledgeKindInput;
    /**
     * 
     */
    body: KnowledgeBodyInput;
    /**
     * 
     */
    sourceRefs: Array<RefInput>;
    /**
     * 
     */
    formed: TimePointInput;
    /**
     * 
     */
    valid?: TimeRangeInput;
    /**
     * 
     */
    formation: KnowledgeViewInputFormationEnum;
    /**
     * 
     */
    stance: KnowledgeViewInputStanceEnum;
    /**
     * 
     */
    confidence?: number;
    /**
     * 
     */
    policy: PolicyInput;
    /**
     * 
     */
    ref: RefInput;
    /**
     * 
     */
    alias: string;
    /**
     * 
     */
    revision: number;
    /**
     * 
     */
    sourceRoots: Array<RefInput>;
    /**
     * 
     */
    recorded: TimePointInput;
    /**
     * 
     */
    status: KnowledgeViewInputStatusEnum;
}


/**
 * @export
 */
export const KnowledgeViewInputFormationEnum = {
    Observed: 'observed',
    Told: 'told',
    Read: 'read',
    Inferred: 'inferred',
    Reflection: 'reflection',
    InitialSetting: 'initial_setting',
    OperatorEdit: 'operator_edit',
} as const;
export type KnowledgeViewInputFormationEnum = typeof KnowledgeViewInputFormationEnum[keyof typeof KnowledgeViewInputFormationEnum];

/**
 * @export
 */
export const KnowledgeViewInputStanceEnum = {
    Accepted: 'accepted',
    Uncertain: 'uncertain',
    Hypothesis: 'hypothesis',
    Disputed: 'disputed',
} as const;
export type KnowledgeViewInputStanceEnum = typeof KnowledgeViewInputStanceEnum[keyof typeof KnowledgeViewInputStanceEnum];

/**
 * @export
 */
export const KnowledgeViewInputStatusEnum = {
    Active: 'active',
    Suppressed: 'suppressed',
    Retired: 'retired',
    Purged: 'purged',
} as const;
export type KnowledgeViewInputStatusEnum = typeof KnowledgeViewInputStatusEnum[keyof typeof KnowledgeViewInputStatusEnum];

/**
 * 
 * @export
 * @interface LanguageConvention
 */
export interface LanguageConvention {
    /**
     * 
     */
    content: string;
    /**
     * 
     */
    title?: string;
    /**
     * 
     */
    entityRefs: Array<Ref>;
    /**
     * 
     */
    conceptRefs: Array<Ref>;
    /**
     * 
     */
    tags: Array<string>;
    /**
     * 
     */
    kind: LanguageConventionKindEnum;
    /**
     * 
     */
    form: string;
    /**
     * 
     */
    meaning: string;
    /**
     * 
     */
    usage: string;
    /**
     * 
     */
    scopeRefs: Array<Ref>;
    /**
     * 
     */
    examples: Array<string>;
}


/**
 * @export
 */
export const LanguageConventionKindEnum = {
    LanguageConvention: 'LanguageConvention',
} as const;
export type LanguageConventionKindEnum = typeof LanguageConventionKindEnum[keyof typeof LanguageConventionKindEnum];

/**
 * 
 * @export
 * @interface LanguageConventionInput
 */
export interface LanguageConventionInput {
    /**
     * 
     */
    content: string;
    /**
     * 
     */
    title?: string;
    /**
     * 
     */
    entityRefs?: Array<RefInput>;
    /**
     * 
     */
    conceptRefs?: Array<RefInput>;
    /**
     * 
     */
    tags?: Array<string>;
    /**
     * 
     */
    kind: LanguageConventionInputKindEnum;
    /**
     * 
     */
    form: string;
    /**
     * 
     */
    meaning: string;
    /**
     * 
     */
    usage: string;
    /**
     * 
     */
    scopeRefs: Array<RefInput>;
    /**
     * 
     */
    examples: Array<string>;
}


/**
 * @export
 */
export const LanguageConventionInputKindEnum = {
    LanguageConvention: 'LanguageConvention',
} as const;
export type LanguageConventionInputKindEnum = typeof LanguageConventionInputKindEnum[keyof typeof LanguageConventionInputKindEnum];

/**
 * 
 * @export
 * @interface MaterialCapture
 */
export interface MaterialCapture {
    /**
     * 
     */
    kind: string;
    /**
     * 
     */
    mediaType: string;
    /**
     * 
     */
    title?: string;
    /**
     * 
     */
    text?: string;
    /**
     * 
     */
    blobRef?: Ref;
    /**
     * 
     */
    source: MaterialSource;
    /**
     * 
     */
    occurred?: TimeRange;
    /**
     * 
     */
    policy?: Policy;
}
/**
 * 
 * @export
 * @interface MaterialCaptureInput
 */
export interface MaterialCaptureInput {
    /**
     * 
     */
    kind: string;
    /**
     * 
     */
    mediaType: string;
    /**
     * 
     */
    title?: string;
    /**
     * 
     */
    text?: string;
    /**
     * 
     */
    blobRef?: RefInput;
    /**
     * 
     */
    source: MaterialSourceInput;
    /**
     * 
     */
    occurred?: TimeRangeInput;
    /**
     * 
     */
    policy?: PolicyInput;
}
/**
 * 
 * @export
 * @interface MaterialPublish
 */
export interface MaterialPublish {
    /**
     * 
     */
    ref: Ref;
    /**
     * 
     */
    title?: string;
}
/**
 * 
 * @export
 * @interface MaterialPublishInput
 */
export interface MaterialPublishInput {
    /**
     * 
     */
    ref: RefInput;
    /**
     * 
     */
    title?: string;
}
/**
 * 
 * @export
 * @interface MaterialSource
 */
export interface MaterialSource {
    /**
     * 
     */
    ref?: Ref;
    /**
     * 
     */
    inputs: Array<Ref>;
    /**
     * 
     */
    roots: Array<Ref>;
    /**
     * 
     */
    originVersion?: string;
}
/**
 * 
 * @export
 * @interface MaterialSourceInput
 */
export interface MaterialSourceInput {
    /**
     * 
     */
    ref?: RefInput;
    /**
     * 
     */
    inputs?: Array<RefInput>;
    /**
     * 
     */
    roots?: Array<RefInput>;
    /**
     * 
     */
    originVersion?: string;
}
/**
 * 
 * @export
 * @interface MaterialView
 */
export interface MaterialView {
    /**
     * 
     */
    kind: string;
    /**
     * 
     */
    mediaType: string;
    /**
     * 
     */
    title?: string;
    /**
     * 
     */
    text?: string;
    /**
     * 
     */
    blobRef?: Ref;
    /**
     * 
     */
    source: MaterialSource;
    /**
     * 
     */
    occurred?: TimeRange;
    /**
     * 
     */
    policy: Policy;
    /**
     * 
     */
    ref: Ref;
    /**
     * 
     */
    scope: Consumer;
    /**
     * 
     */
    recordedAt: string;
}
/**
 * 
 * @export
 * @interface MaterialViewInput
 */
export interface MaterialViewInput {
    /**
     * 
     */
    kind: string;
    /**
     * 
     */
    mediaType: string;
    /**
     * 
     */
    title?: string;
    /**
     * 
     */
    text?: string;
    /**
     * 
     */
    blobRef?: RefInput;
    /**
     * 
     */
    source: MaterialSourceInput;
    /**
     * 
     */
    occurred?: TimeRangeInput;
    /**
     * 
     */
    policy: PolicyInput;
    /**
     * 
     */
    ref: RefInput;
    /**
     * 
     */
    scope: ConsumerInput;
    /**
     * 
     */
    recordedAt: string;
}
/**
 * 
 * @export
 * @interface MaterialsUploadBlob200Response
 */
export interface MaterialsUploadBlob200Response {
    /**
     * 
     */
    ref: Ref;
    /**
     * 
     */
    digest: string;
    /**
     * 
     */
    bytes: number;
}
/**
 * 
 * @export
 * @interface Memory
 */
export interface Memory {
    /**
     * 
     */
    content: string;
    /**
     * 
     */
    title?: string;
    /**
     * 
     */
    entityRefs: Array<Ref>;
    /**
     * 
     */
    conceptRefs: Array<Ref>;
    /**
     * 
     */
    tags: Array<string>;
    /**
     * 
     */
    kind: MemoryKindEnum;
    /**
     * 
     */
    role: MemoryRoleEnum;
}


/**
 * @export
 */
export const MemoryKindEnum = {
    Memory: 'Memory',
} as const;
export type MemoryKindEnum = typeof MemoryKindEnum[keyof typeof MemoryKindEnum];

/**
 * @export
 */
export const MemoryRoleEnum = {
    Experience: 'experience',
    Fact: 'fact',
    Reported: 'reported',
    Preference: 'preference',
    Procedure: 'procedure',
    Interpretation: 'interpretation',
} as const;
export type MemoryRoleEnum = typeof MemoryRoleEnum[keyof typeof MemoryRoleEnum];

/**
 * 
 * @export
 * @interface MemoryInput
 */
export interface MemoryInput {
    /**
     * 
     */
    content: string;
    /**
     * 
     */
    title?: string;
    /**
     * 
     */
    entityRefs?: Array<RefInput>;
    /**
     * 
     */
    conceptRefs?: Array<RefInput>;
    /**
     * 
     */
    tags?: Array<string>;
    /**
     * 
     */
    kind: MemoryInputKindEnum;
    /**
     * 
     */
    role?: MemoryInputRoleEnum;
}


/**
 * @export
 */
export const MemoryInputKindEnum = {
    Memory: 'Memory',
} as const;
export type MemoryInputKindEnum = typeof MemoryInputKindEnum[keyof typeof MemoryInputKindEnum];

/**
 * @export
 */
export const MemoryInputRoleEnum = {
    Experience: 'experience',
    Fact: 'fact',
    Reported: 'reported',
    Preference: 'preference',
    Procedure: 'procedure',
    Interpretation: 'interpretation',
} as const;
export type MemoryInputRoleEnum = typeof MemoryInputRoleEnum[keyof typeof MemoryInputRoleEnum];

/**
 * 
 * @export
 * @interface ModelResource
 */
export interface ModelResource {
    /**
     * 
     */
    id: string;
    /**
     * 
     */
    provider: ModelResourceProviderEnum;
    /**
     * 
     */
    modelId: string;
    /**
     * 
     */
    baseURL: string;
    /**
     * 
     */
    credentialRef: string;
    /**
     * 
     */
    capabilities: Array<ModelResourceCapabilitiesEnum>;
    /**
     * 
     */
    contextLimit: number;
    /**
     * 
     */
    outputLimit: number;
    /**
     * 
     */
    pricing: ModelResourcePricing | null;
    /**
     * 
     */
    providerOptions: { [key: string]: any; };
}


/**
 * @export
 */
export const ModelResourceProviderEnum = {
    OpenaiCompatible: 'openai-compatible',
} as const;
export type ModelResourceProviderEnum = typeof ModelResourceProviderEnum[keyof typeof ModelResourceProviderEnum];

/**
 * @export
 */
export const ModelResourceCapabilitiesEnum = {
    Text: 'text',
    Vision: 'vision',
    Tools: 'tools',
    Structured: 'structured',
    Embedding: 'embedding',
} as const;
export type ModelResourceCapabilitiesEnum = typeof ModelResourceCapabilitiesEnum[keyof typeof ModelResourceCapabilitiesEnum];

/**
 * 
 * @export
 * @interface ModelResourceInput
 */
export interface ModelResourceInput {
    /**
     * 
     */
    id: string;
    /**
     * 
     */
    provider: ModelResourceInputProviderEnum;
    /**
     * 
     */
    modelId: string;
    /**
     * 
     */
    baseURL: string;
    /**
     * 
     */
    credentialRef: string;
    /**
     * 
     */
    capabilities: Array<ModelResourceInputCapabilitiesEnum>;
    /**
     * 
     */
    contextLimit: number;
    /**
     * 
     */
    outputLimit: number;
    /**
     * 
     */
    pricing?: ModelResourceInputPricing | null;
    /**
     * 
     */
    providerOptions?: { [key: string]: any; };
}


/**
 * @export
 */
export const ModelResourceInputProviderEnum = {
    OpenaiCompatible: 'openai-compatible',
} as const;
export type ModelResourceInputProviderEnum = typeof ModelResourceInputProviderEnum[keyof typeof ModelResourceInputProviderEnum];

/**
 * @export
 */
export const ModelResourceInputCapabilitiesEnum = {
    Text: 'text',
    Vision: 'vision',
    Tools: 'tools',
    Structured: 'structured',
    Embedding: 'embedding',
} as const;
export type ModelResourceInputCapabilitiesEnum = typeof ModelResourceInputCapabilitiesEnum[keyof typeof ModelResourceInputCapabilitiesEnum];

/**
 * 
 * @export
 * @interface ModelResourceInputPricing
 */
export interface ModelResourceInputPricing {
    /**
     * 
     */
    inputPerMillion: number;
    /**
     * 
     */
    outputPerMillion: number;
    /**
     * 
     */
    currency?: string;
}
/**
 * 
 * @export
 * @interface ModelResourcePricing
 */
export interface ModelResourcePricing {
    /**
     * 
     */
    inputPerMillion: number;
    /**
     * 
     */
    outputPerMillion: number;
    /**
     * 
     */
    currency: string;
}
/**
 * 
 * @export
 * @interface ModelsList200Response
 */
export interface ModelsList200Response {
    /**
     * 
     */
    items: Array<ModelResource>;
}
/**
 * 
 * @export
 * @interface Motivation
 */
export interface Motivation {
    /**
     * 
     */
    content: string;
    /**
     * 
     */
    title?: string;
    /**
     * 
     */
    entityRefs: Array<Ref>;
    /**
     * 
     */
    conceptRefs: Array<Ref>;
    /**
     * 
     */
    tags: Array<string>;
    /**
     * 
     */
    kind: MotivationKindEnum;
    /**
     * 
     */
    aspect: MotivationAspectEnum;
    /**
     * 
     */
    importance: number;
    /**
     * 
     */
    criterion?: Criterion;
}


/**
 * @export
 */
export const MotivationKindEnum = {
    Motivation: 'Motivation',
} as const;
export type MotivationKindEnum = typeof MotivationKindEnum[keyof typeof MotivationKindEnum];

/**
 * @export
 */
export const MotivationAspectEnum = {
    Desire: 'desire',
    Concern: 'concern',
    EvaluableExpectation: 'evaluable_expectation',
} as const;
export type MotivationAspectEnum = typeof MotivationAspectEnum[keyof typeof MotivationAspectEnum];

/**
 * 
 * @export
 * @interface MotivationEvaluateRequest
 */
export interface MotivationEvaluateRequest {
    /**
     * 
     */
    revision?: number;
    /**
     * 
     */
    observationRefs: Array<RefInput>;
    /**
     * 
     */
    coverage: MotivationEvaluateRequestCoverage;
    /**
     * 
     */
    evaluatedAt: TimePointInput;
}
/**
 * 
 * @export
 * @interface MotivationEvaluateRequestCoverage
 */
export interface MotivationEvaluateRequestCoverage {
    /**
     * 
     */
    range: TimeRangeInput;
    /**
     * 
     */
    ratio: number;
    /**
     * 
     */
    sourceRefs: Array<RefInput>;
    /**
     * 
     */
    missing?: Array<TimeRangeInput>;
}
/**
 * 
 * @export
 * @interface MotivationInput
 */
export interface MotivationInput {
    /**
     * 
     */
    content: string;
    /**
     * 
     */
    title?: string;
    /**
     * 
     */
    entityRefs?: Array<RefInput>;
    /**
     * 
     */
    conceptRefs?: Array<RefInput>;
    /**
     * 
     */
    tags?: Array<string>;
    /**
     * 
     */
    kind: MotivationInputKindEnum;
    /**
     * 
     */
    aspect: MotivationInputAspectEnum;
    /**
     * 
     */
    importance: number;
    /**
     * 
     */
    criterion?: CriterionInput;
}


/**
 * @export
 */
export const MotivationInputKindEnum = {
    Motivation: 'Motivation',
} as const;
export type MotivationInputKindEnum = typeof MotivationInputKindEnum[keyof typeof MotivationInputKindEnum];

/**
 * @export
 */
export const MotivationInputAspectEnum = {
    Desire: 'desire',
    Concern: 'concern',
    EvaluableExpectation: 'evaluable_expectation',
} as const;
export type MotivationInputAspectEnum = typeof MotivationInputAspectEnum[keyof typeof MotivationInputAspectEnum];

/**
 * 
 * @export
 * @interface ObjectRelation
 */
export interface ObjectRelation {
    /**
     * 
     */
    fromId: string;
    /**
     * 
     */
    toId: string;
    /**
     * 
     */
    relation: string;
    /**
     * 
     */
    weight: number;
    /**
     * 
     */
    evidenceRefs: Array<Ref>;
    /**
     * 
     */
    formed: TimePoint;
    /**
     * 
     */
    valid?: TimeRange | null;
    /**
     * 
     */
    id: string;
    /**
     * 
     */
    relationId: string;
    /**
     * 
     */
    revision: number;
    /**
     * 
     */
    subjectId: string;
    /**
     * 
     */
    roots: Array<Ref>;
    /**
     * 
     */
    recordedAt: string;
}
/**
 * 
 * @export
 * @interface ObjectRelationInput
 */
export interface ObjectRelationInput {
    /**
     * 
     */
    fromId: string;
    /**
     * 
     */
    toId: string;
    /**
     * 
     */
    relation: string;
    /**
     * 
     */
    weight?: number;
    /**
     * 
     */
    evidenceRefs: Array<RefInput>;
    /**
     * 
     */
    formed: TimePointInput;
    /**
     * 
     */
    valid?: TimeRangeInput | null;
    /**
     * 
     */
    id: string;
    /**
     * 
     */
    relationId: string;
    /**
     * 
     */
    revision: number;
    /**
     * 
     */
    subjectId: string;
    /**
     * 
     */
    roots: Array<RefInput>;
    /**
     * 
     */
    recordedAt: string;
}
/**
 * 
 * @export
 * @interface OperationRef
 */
export interface OperationRef {
    /**
     * 
     */
    operationId: string;
    /**
     * 
     */
    statusUrl: string;
    /**
     * 
     */
    resourceRef?: Ref;
}
/**
 * 
 * @export
 * @interface OperationRefInput
 */
export interface OperationRefInput {
    /**
     * 
     */
    operationId: string;
    /**
     * 
     */
    statusUrl: string;
    /**
     * 
     */
    resourceRef?: RefInput;
}
/**
 * 
 * @export
 * @interface Policy
 */
export interface Policy {
    /**
     * 
     */
    readers: Array<string>;
    /**
     * 
     */
    purposes: Array<string>;
    /**
     * 
     */
    audiences: Array<string>;
}
/**
 * 
 * @export
 * @interface PolicyInput
 */
export interface PolicyInput {
    /**
     * 
     */
    readers: Array<string>;
    /**
     * 
     */
    purposes: Array<string>;
    /**
     * 
     */
    audiences: Array<string>;
}
/**
 * 
 * @export
 * @interface PolicyUpdate200Response
 */
export interface PolicyUpdate200Response {
    /**
     * 
     */
    ref: Ref;
    /**
     * 
     */
    policy: Policy;
}
/**
 * 
 * @export
 * @interface Portion
 */
export interface Portion {
    /**
     * 
     */
    ref: Ref;
    /**
     * 
     */
    coverage?: string;
}
/**
 * 
 * @export
 * @interface PortionInput
 */
export interface PortionInput {
    /**
     * 
     */
    ref: RefInput;
    /**
     * 
     */
    coverage?: string;
}
/**
 * 
 * @export
 * @interface Problem
 */
export interface Problem {
    /**
     * 
     */
    type: string;
    /**
     * 
     */
    title: string;
    /**
     * 
     */
    status: number;
    /**
     * 
     */
    detail?: string;
    /**
     * 
     */
    code: ProblemCodeEnum;
    /**
     * 
     */
    operationId?: string;
    /**
     * 
     */
    currentRevision?: number;
    /**
     * 
     */
    retryAfter?: number;
}


/**
 * @export
 */
export const ProblemCodeEnum = {
    InvalidInput: 'invalid_input',
    Forbidden: 'forbidden',
    NotFound: 'not_found',
    Conflict: 'conflict',
    CapabilityUnavailable: 'capability_unavailable',
    BudgetExhausted: 'budget_exhausted',
    DependencyUnavailable: 'dependency_unavailable',
    EffectUnknown: 'effect_unknown',
} as const;
export type ProblemCodeEnum = typeof ProblemCodeEnum[keyof typeof ProblemCodeEnum];

/**
 * 
 * @export
 * @interface ProblemInput
 */
export interface ProblemInput {
    /**
     * 
     */
    type: string;
    /**
     * 
     */
    title: string;
    /**
     * 
     */
    status: number;
    /**
     * 
     */
    detail?: string;
    /**
     * 
     */
    code: ProblemInputCodeEnum;
    /**
     * 
     */
    operationId?: string;
    /**
     * 
     */
    currentRevision?: number;
    /**
     * 
     */
    retryAfter?: number;
}


/**
 * @export
 */
export const ProblemInputCodeEnum = {
    InvalidInput: 'invalid_input',
    Forbidden: 'forbidden',
    NotFound: 'not_found',
    Conflict: 'conflict',
    CapabilityUnavailable: 'capability_unavailable',
    BudgetExhausted: 'budget_exhausted',
    DependencyUnavailable: 'dependency_unavailable',
    EffectUnknown: 'effect_unknown',
} as const;
export type ProblemInputCodeEnum = typeof ProblemInputCodeEnum[keyof typeof ProblemInputCodeEnum];

/**
 * 
 * @export
 * @interface Query
 */
export interface Query {
    /**
     * 
     */
    intent: string;
    /**
     * 
     */
    subjectId?: string;
    /**
     * 
     */
    kinds?: Array<KnowledgeKind>;
    /**
     * 
     */
    entities: Array<string>;
    /**
     * 
     */
    entityMatch: QueryEntityMatchEnum;
    /**
     * 
     */
    concepts: Array<string>;
    /**
     * 
     */
    tags: Array<string>;
    /**
     * 
     */
    tagMatch: QueryTagMatchEnum;
    /**
     * 
     */
    time?: QueryTime;
    /**
     * 
     */
    knowledgeAt?: TimePoint;
    /**
     * 
     */
    recordedAsOf?: string;
    /**
     * 
     */
    refs: Array<string>;
    /**
     * 
     */
    sources: Array<string>;
    /**
     * 
     */
    preferences: Array<QueryPreferencesEnum>;
    /**
     * 
     */
    exploration: number;
    /**
     * 
     */
    residual: boolean;
    /**
     * 
     */
    expand: Array<QueryExpandEnum>;
    /**
     * 
     */
    limit: number;
    /**
     * 
     */
    cursor?: string;
    /**
     * 
     */
    workContextId?: string;
}


/**
 * @export
 */
export const QueryEntityMatchEnum = {
    Any: 'any',
    All: 'all',
} as const;
export type QueryEntityMatchEnum = typeof QueryEntityMatchEnum[keyof typeof QueryEntityMatchEnum];

/**
 * @export
 */
export const QueryTagMatchEnum = {
    Any: 'any',
    All: 'all',
} as const;
export type QueryTagMatchEnum = typeof QueryTagMatchEnum[keyof typeof QueryTagMatchEnum];

/**
 * @export
 */
export const QueryPreferencesEnum = {
    Recent: 'recent',
    IndependentSources: 'independent_sources',
    Diverse: 'diverse',
} as const;
export type QueryPreferencesEnum = typeof QueryPreferencesEnum[keyof typeof QueryPreferencesEnum];

/**
 * @export
 */
export const QueryExpandEnum = {
    Sources: 'sources',
    Relations: 'relations',
} as const;
export type QueryExpandEnum = typeof QueryExpandEnum[keyof typeof QueryExpandEnum];

/**
 * 
 * @export
 * @interface QueryExplainRequest
 */
export interface QueryExplainRequest {
    /**
     * 
     */
    intent?: string;
    /**
     * 
     */
    subjectId?: string;
    /**
     * 
     */
    kinds?: Array<KnowledgeKindInput>;
    /**
     * 
     */
    entities?: Array<string>;
    /**
     * 
     */
    entityMatch?: QueryExplainRequestEntityMatchEnum;
    /**
     * 
     */
    concepts?: Array<string>;
    /**
     * 
     */
    tags?: Array<string>;
    /**
     * 
     */
    tagMatch?: QueryExplainRequestTagMatchEnum;
    /**
     * 
     */
    time?: QueryInputTime;
    /**
     * 
     */
    knowledgeAt?: TimePointInput;
    /**
     * 
     */
    recordedAsOf?: string;
    /**
     * 
     */
    refs?: Array<string>;
    /**
     * 
     */
    sources?: Array<string>;
    /**
     * 
     */
    preferences?: Array<QueryExplainRequestPreferencesEnum>;
    /**
     * 
     */
    exploration?: number;
    /**
     * 
     */
    residual?: boolean;
    /**
     * 
     */
    expand?: Array<QueryExplainRequestExpandEnum>;
    /**
     * 
     */
    limit?: number;
    /**
     * 
     */
    cursor?: string;
    /**
     * 
     */
    workContextId?: string;
}


/**
 * @export
 */
export const QueryExplainRequestEntityMatchEnum = {
    Any: 'any',
    All: 'all',
} as const;
export type QueryExplainRequestEntityMatchEnum = typeof QueryExplainRequestEntityMatchEnum[keyof typeof QueryExplainRequestEntityMatchEnum];

/**
 * @export
 */
export const QueryExplainRequestTagMatchEnum = {
    Any: 'any',
    All: 'all',
} as const;
export type QueryExplainRequestTagMatchEnum = typeof QueryExplainRequestTagMatchEnum[keyof typeof QueryExplainRequestTagMatchEnum];

/**
 * @export
 */
export const QueryExplainRequestPreferencesEnum = {
    Recent: 'recent',
    IndependentSources: 'independent_sources',
    Diverse: 'diverse',
} as const;
export type QueryExplainRequestPreferencesEnum = typeof QueryExplainRequestPreferencesEnum[keyof typeof QueryExplainRequestPreferencesEnum];

/**
 * @export
 */
export const QueryExplainRequestExpandEnum = {
    Sources: 'sources',
    Relations: 'relations',
} as const;
export type QueryExplainRequestExpandEnum = typeof QueryExplainRequestExpandEnum[keyof typeof QueryExplainRequestExpandEnum];

/**
 * 
 * @export
 * @interface QueryInput
 */
export interface QueryInput {
    /**
     * 
     */
    intent?: string;
    /**
     * 
     */
    subjectId?: string;
    /**
     * 
     */
    kinds?: Array<KnowledgeKindInput>;
    /**
     * 
     */
    entities?: Array<string>;
    /**
     * 
     */
    entityMatch?: QueryInputEntityMatchEnum;
    /**
     * 
     */
    concepts?: Array<string>;
    /**
     * 
     */
    tags?: Array<string>;
    /**
     * 
     */
    tagMatch?: QueryInputTagMatchEnum;
    /**
     * 
     */
    time?: QueryInputTime;
    /**
     * 
     */
    knowledgeAt?: TimePointInput;
    /**
     * 
     */
    recordedAsOf?: string;
    /**
     * 
     */
    refs?: Array<string>;
    /**
     * 
     */
    sources?: Array<string>;
    /**
     * 
     */
    preferences?: Array<QueryInputPreferencesEnum>;
    /**
     * 
     */
    exploration?: number;
    /**
     * 
     */
    residual?: boolean;
    /**
     * 
     */
    expand?: Array<QueryInputExpandEnum>;
    /**
     * 
     */
    limit?: number;
    /**
     * 
     */
    cursor?: string;
    /**
     * 
     */
    workContextId?: string;
}


/**
 * @export
 */
export const QueryInputEntityMatchEnum = {
    Any: 'any',
    All: 'all',
} as const;
export type QueryInputEntityMatchEnum = typeof QueryInputEntityMatchEnum[keyof typeof QueryInputEntityMatchEnum];

/**
 * @export
 */
export const QueryInputTagMatchEnum = {
    Any: 'any',
    All: 'all',
} as const;
export type QueryInputTagMatchEnum = typeof QueryInputTagMatchEnum[keyof typeof QueryInputTagMatchEnum];

/**
 * @export
 */
export const QueryInputPreferencesEnum = {
    Recent: 'recent',
    IndependentSources: 'independent_sources',
    Diverse: 'diverse',
} as const;
export type QueryInputPreferencesEnum = typeof QueryInputPreferencesEnum[keyof typeof QueryInputPreferencesEnum];

/**
 * @export
 */
export const QueryInputExpandEnum = {
    Sources: 'sources',
    Relations: 'relations',
} as const;
export type QueryInputExpandEnum = typeof QueryInputExpandEnum[keyof typeof QueryInputExpandEnum];

/**
 * 
 * @export
 * @interface QueryInputTime
 */
export interface QueryInputTime {
    /**
     * 
     */
    field: QueryInputTimeFieldEnum;
    /**
     * 
     */
    clockId: string;
    /**
     * 
     */
    start: string;
    /**
     * 
     */
    end: string;
}


/**
 * @export
 */
export const QueryInputTimeFieldEnum = {
    Occurred: 'occurred',
    Observed: 'observed',
    Formed: 'formed',
    Valid: 'valid',
    Recorded: 'recorded',
} as const;
export type QueryInputTimeFieldEnum = typeof QueryInputTimeFieldEnum[keyof typeof QueryInputTimeFieldEnum];

/**
 * 
 * @export
 * @interface QueryItem
 */
export interface QueryItem {
    /**
     * 
     */
    ref: Ref;
    /**
     * 
     */
    title: string;
    /**
     * 
     */
    excerpt: string;
    /**
     * 
     */
    formation: string;
    /**
     * 
     */
    stance: string;
    /**
     * 
     */
    confidence?: number;
    /**
     * 
     */
    time: QueryItemTime;
    /**
     * 
     */
    sourceRefs: Array<Ref>;
    /**
     * 
     */
    matchReasons: Array<string>;
    /**
     * 
     */
    relatedRefs?: Array<Ref>;
}
/**
 * 
 * @export
 * @interface QueryItemInput
 */
export interface QueryItemInput {
    /**
     * 
     */
    ref: RefInput;
    /**
     * 
     */
    title: string;
    /**
     * 
     */
    excerpt: string;
    /**
     * 
     */
    formation: string;
    /**
     * 
     */
    stance: string;
    /**
     * 
     */
    confidence?: number;
    /**
     * 
     */
    time: QueryItemInputTime;
    /**
     * 
     */
    sourceRefs: Array<RefInput>;
    /**
     * 
     */
    matchReasons: Array<string>;
    /**
     * 
     */
    relatedRefs?: Array<RefInput>;
}
/**
 * 
 * @export
 * @interface QueryItemInputTime
 */
export interface QueryItemInputTime {
    /**
     * 
     */
    formed: TimePointInput;
    /**
     * 
     */
    valid?: TimeRangeInput;
    /**
     * 
     */
    recorded: TimePointInput;
}
/**
 * 
 * @export
 * @interface QueryItemTime
 */
export interface QueryItemTime {
    /**
     * 
     */
    formed: TimePoint;
    /**
     * 
     */
    valid?: TimeRange;
    /**
     * 
     */
    recorded: TimePoint;
}
/**
 * 
 * @export
 * @interface QueryRequest
 */
export interface QueryRequest {
    /**
     * 
     */
    intent: string;
    /**
     * 
     */
    subjectId?: string;
    /**
     * 
     */
    kinds?: Array<KnowledgeKind>;
    /**
     * 
     */
    entities: Array<string>;
    /**
     * 
     */
    entityMatch: QueryRequestEntityMatchEnum;
    /**
     * 
     */
    concepts: Array<string>;
    /**
     * 
     */
    tags: Array<string>;
    /**
     * 
     */
    tagMatch: QueryRequestTagMatchEnum;
    /**
     * 
     */
    time?: QueryTime;
    /**
     * 
     */
    knowledgeAt?: TimePoint;
    /**
     * 
     */
    recordedAsOf?: string;
    /**
     * 
     */
    refs: Array<string>;
    /**
     * 
     */
    sources: Array<string>;
    /**
     * 
     */
    preferences: Array<QueryRequestPreferencesEnum>;
    /**
     * 
     */
    exploration: number;
    /**
     * 
     */
    residual: boolean;
    /**
     * 
     */
    expand: Array<QueryRequestExpandEnum>;
    /**
     * 
     */
    limit: number;
    /**
     * 
     */
    cursor?: string;
    /**
     * 
     */
    workContextId?: string;
}


/**
 * @export
 */
export const QueryRequestEntityMatchEnum = {
    Any: 'any',
    All: 'all',
} as const;
export type QueryRequestEntityMatchEnum = typeof QueryRequestEntityMatchEnum[keyof typeof QueryRequestEntityMatchEnum];

/**
 * @export
 */
export const QueryRequestTagMatchEnum = {
    Any: 'any',
    All: 'all',
} as const;
export type QueryRequestTagMatchEnum = typeof QueryRequestTagMatchEnum[keyof typeof QueryRequestTagMatchEnum];

/**
 * @export
 */
export const QueryRequestPreferencesEnum = {
    Recent: 'recent',
    IndependentSources: 'independent_sources',
    Diverse: 'diverse',
} as const;
export type QueryRequestPreferencesEnum = typeof QueryRequestPreferencesEnum[keyof typeof QueryRequestPreferencesEnum];

/**
 * @export
 */
export const QueryRequestExpandEnum = {
    Sources: 'sources',
    Relations: 'relations',
} as const;
export type QueryRequestExpandEnum = typeof QueryRequestExpandEnum[keyof typeof QueryRequestExpandEnum];

/**
 * 
 * @export
 * @interface QueryRequestInput
 */
export interface QueryRequestInput {
    /**
     * 
     */
    intent?: string;
    /**
     * 
     */
    subjectId?: string;
    /**
     * 
     */
    kinds?: Array<KnowledgeKindInput>;
    /**
     * 
     */
    entities?: Array<string>;
    /**
     * 
     */
    entityMatch?: QueryRequestInputEntityMatchEnum;
    /**
     * 
     */
    concepts?: Array<string>;
    /**
     * 
     */
    tags?: Array<string>;
    /**
     * 
     */
    tagMatch?: QueryRequestInputTagMatchEnum;
    /**
     * 
     */
    time?: QueryInputTime;
    /**
     * 
     */
    knowledgeAt?: TimePointInput;
    /**
     * 
     */
    recordedAsOf?: string;
    /**
     * 
     */
    refs?: Array<string>;
    /**
     * 
     */
    sources?: Array<string>;
    /**
     * 
     */
    preferences?: Array<QueryRequestInputPreferencesEnum>;
    /**
     * 
     */
    exploration?: number;
    /**
     * 
     */
    residual?: boolean;
    /**
     * 
     */
    expand?: Array<QueryRequestInputExpandEnum>;
    /**
     * 
     */
    limit?: number;
    /**
     * 
     */
    cursor?: string;
    /**
     * 
     */
    workContextId?: string;
}


/**
 * @export
 */
export const QueryRequestInputEntityMatchEnum = {
    Any: 'any',
    All: 'all',
} as const;
export type QueryRequestInputEntityMatchEnum = typeof QueryRequestInputEntityMatchEnum[keyof typeof QueryRequestInputEntityMatchEnum];

/**
 * @export
 */
export const QueryRequestInputTagMatchEnum = {
    Any: 'any',
    All: 'all',
} as const;
export type QueryRequestInputTagMatchEnum = typeof QueryRequestInputTagMatchEnum[keyof typeof QueryRequestInputTagMatchEnum];

/**
 * @export
 */
export const QueryRequestInputPreferencesEnum = {
    Recent: 'recent',
    IndependentSources: 'independent_sources',
    Diverse: 'diverse',
} as const;
export type QueryRequestInputPreferencesEnum = typeof QueryRequestInputPreferencesEnum[keyof typeof QueryRequestInputPreferencesEnum];

/**
 * @export
 */
export const QueryRequestInputExpandEnum = {
    Sources: 'sources',
    Relations: 'relations',
} as const;
export type QueryRequestInputExpandEnum = typeof QueryRequestInputExpandEnum[keyof typeof QueryRequestInputExpandEnum];

/**
 * 
 * @export
 * @interface QueryResult
 */
export interface QueryResult {
    /**
     * 
     */
    items: Array<QueryItem>;
    /**
     * 
     */
    resolvedQuery: Query;
    /**
     * 
     */
    nextCursor?: string;
    /**
     * 
     */
    resultSetId: string;
    /**
     * 
     */
    indexState: QueryResultIndexState;
}
/**
 * 
 * @export
 * @interface QueryResultIndexState
 */
export interface QueryResultIndexState {
    /**
     * 
     */
    textReady: boolean;
    /**
     * 
     */
    vectorReady: boolean;
    /**
     * 
     */
    embeddingSpace?: string;
    /**
     * 
     */
    tokenizer: QueryResultIndexStateTokenizerEnum;
    /**
     * 
     */
    selectedRevisions: number;
    /**
     * 
     */
    historical: boolean;
}


/**
 * @export
 */
export const QueryResultIndexStateTokenizerEnum = {
    Icu: 'icu',
} as const;
export type QueryResultIndexStateTokenizerEnum = typeof QueryResultIndexStateTokenizerEnum[keyof typeof QueryResultIndexStateTokenizerEnum];

/**
 * 
 * @export
 * @interface QueryResultInput
 */
export interface QueryResultInput {
    /**
     * 
     */
    items: Array<QueryItemInput>;
    /**
     * 
     */
    resolvedQuery: QueryInput;
    /**
     * 
     */
    nextCursor?: string;
    /**
     * 
     */
    resultSetId: string;
    /**
     * 
     */
    indexState: QueryResultInputIndexState;
}
/**
 * 
 * @export
 * @interface QueryResultInputIndexState
 */
export interface QueryResultInputIndexState {
    /**
     * 
     */
    textReady: boolean;
    /**
     * 
     */
    vectorReady: boolean;
    /**
     * 
     */
    embeddingSpace?: string;
    /**
     * 
     */
    tokenizer: QueryResultInputIndexStateTokenizerEnum;
    /**
     * 
     */
    selectedRevisions: number;
    /**
     * 
     */
    historical: boolean;
}


/**
 * @export
 */
export const QueryResultInputIndexStateTokenizerEnum = {
    Icu: 'icu',
} as const;
export type QueryResultInputIndexStateTokenizerEnum = typeof QueryResultInputIndexStateTokenizerEnum[keyof typeof QueryResultInputIndexStateTokenizerEnum];

/**
 * 
 * @export
 * @interface QueryTime
 */
export interface QueryTime {
    /**
     * 
     */
    field: QueryTimeFieldEnum;
    /**
     * 
     */
    clockId: string;
    /**
     * 
     */
    start: string;
    /**
     * 
     */
    end: string;
}


/**
 * @export
 */
export const QueryTimeFieldEnum = {
    Occurred: 'occurred',
    Observed: 'observed',
    Formed: 'formed',
    Valid: 'valid',
    Recorded: 'recorded',
} as const;
export type QueryTimeFieldEnum = typeof QueryTimeFieldEnum[keyof typeof QueryTimeFieldEnum];

/**
 * 
 * @export
 * @interface Ref
 */
export interface Ref {
    /**
     * 
     */
    product: RefProductEnum;
    /**
     * 
     */
    installationId: string;
    /**
     * 
     */
    subjectId?: string;
    /**
     * 
     */
    kind: string;
    /**
     * 
     */
    id: string;
    /**
     * 
     */
    revision?: number;
    /**
     * 
     */
    selector?: Selector;
}


/**
 * @export
 */
export const RefProductEnum = {
    Nous: 'nous',
    Hept: 'hept',
} as const;
export type RefProductEnum = typeof RefProductEnum[keyof typeof RefProductEnum];

/**
 * 
 * @export
 * @interface RefInput
 */
export interface RefInput {
    /**
     * 
     */
    product: RefInputProductEnum;
    /**
     * 
     */
    installationId: string;
    /**
     * 
     */
    subjectId?: string;
    /**
     * 
     */
    kind: string;
    /**
     * 
     */
    id: string;
    /**
     * 
     */
    revision?: number;
    /**
     * 
     */
    selector?: SelectorInput;
}


/**
 * @export
 */
export const RefInputProductEnum = {
    Nous: 'nous',
    Hept: 'hept',
} as const;
export type RefInputProductEnum = typeof RefInputProductEnum[keyof typeof RefInputProductEnum];

/**
 * 
 * @export
 * @interface RelationPage
 */
export interface RelationPage {
    /**
     * 
     */
    items: Array<ObjectRelation>;
    /**
     * 
     */
    nextCursor?: string;
}
/**
 * 
 * @export
 * @interface RelationPageInput
 */
export interface RelationPageInput {
    /**
     * 
     */
    items: Array<ObjectRelationInput>;
    /**
     * 
     */
    nextCursor?: string;
}
/**
 * 
 * @export
 * @interface RelationRead
 */
export interface RelationRead {
    /**
     * 
     */
    knowledgeAt?: TimePoint;
    /**
     * 
     */
    recordedAsOf?: string;
    /**
     * 
     */
    limit: number;
    /**
     * 
     */
    cursor?: string;
}
/**
 * 
 * @export
 * @interface RelationReadInput
 */
export interface RelationReadInput {
    /**
     * 
     */
    knowledgeAt?: TimePointInput;
    /**
     * 
     */
    recordedAsOf?: string;
    /**
     * 
     */
    limit?: number;
    /**
     * 
     */
    cursor?: string;
}
/**
 * 
 * @export
 * @interface RelationWrite
 */
export interface RelationWrite {
    /**
     * 
     */
    fromId: string;
    /**
     * 
     */
    toId: string;
    /**
     * 
     */
    relation: string;
    /**
     * 
     */
    weight: number;
    /**
     * 
     */
    evidenceRefs: Array<Ref>;
    /**
     * 
     */
    formed: TimePoint;
    /**
     * 
     */
    valid?: TimeRange;
}
/**
 * 
 * @export
 * @interface RelationWriteInput
 */
export interface RelationWriteInput {
    /**
     * 
     */
    fromId: string;
    /**
     * 
     */
    toId: string;
    /**
     * 
     */
    relation: string;
    /**
     * 
     */
    weight?: number;
    /**
     * 
     */
    evidenceRefs: Array<RefInput>;
    /**
     * 
     */
    formed: TimePointInput;
    /**
     * 
     */
    valid?: TimeRangeInput;
}
/**
 * @type ResourceBody
 * 
 * @export
 */
export type ResourceBody = ResourceBodyOneOf | ResourceBodyOneOf1 | ResourceBodyOneOf2;
/**
 * @type ResourceBodyInput
 * 
 * @export
 */
export type ResourceBodyInput = ResourceBodyInputOneOf | ResourceBodyInputOneOf1 | ResourceBodyInputOneOf2;
/**
 * 
 * @export
 * @interface ResourceBodyInputOneOf
 */
export interface ResourceBodyInputOneOf {
    /**
     * 
     */
    kind: ResourceBodyInputOneOfKindEnum;
    /**
     * 
     */
    title: string;
    /**
     * 
     */
    root: string;
    /**
     * 
     */
    extensions?: Array<string>;
    /**
     * 
     */
    maxBytes?: number;
    /**
     * 
     */
    policy: PolicyInput;
}


/**
 * @export
 */
export const ResourceBodyInputOneOfKindEnum = {
    Directory: 'directory',
} as const;
export type ResourceBodyInputOneOfKindEnum = typeof ResourceBodyInputOneOfKindEnum[keyof typeof ResourceBodyInputOneOfKindEnum];

/**
 * 
 * @export
 * @interface ResourceBodyInputOneOf1
 */
export interface ResourceBodyInputOneOf1 {
    /**
     * 
     */
    kind: ResourceBodyInputOneOf1KindEnum;
    /**
     * 
     */
    title: string;
    /**
     * 
     */
    baseUrl: string;
    /**
     * 
     */
    allowedOrigins?: Array<string>;
    /**
     * 
     */
    maxBytes?: number;
    /**
     * 
     */
    policy: PolicyInput;
}


/**
 * @export
 */
export const ResourceBodyInputOneOf1KindEnum = {
    Http: 'http',
} as const;
export type ResourceBodyInputOneOf1KindEnum = typeof ResourceBodyInputOneOf1KindEnum[keyof typeof ResourceBodyInputOneOf1KindEnum];

/**
 * 
 * @export
 * @interface ResourceBodyInputOneOf2
 */
export interface ResourceBodyInputOneOf2 {
    /**
     * 
     */
    kind: ResourceBodyInputOneOf2KindEnum;
    /**
     * 
     */
    title: string;
    /**
     * 
     */
    connectorId: string;
    /**
     * 
     */
    allowedUris?: Array<string>;
    /**
     * 
     */
    maxBytes?: number;
    /**
     * 
     */
    policy: PolicyInput;
}


/**
 * @export
 */
export const ResourceBodyInputOneOf2KindEnum = {
    Mcp: 'mcp',
} as const;
export type ResourceBodyInputOneOf2KindEnum = typeof ResourceBodyInputOneOf2KindEnum[keyof typeof ResourceBodyInputOneOf2KindEnum];

/**
 * 
 * @export
 * @interface ResourceBodyOneOf
 */
export interface ResourceBodyOneOf {
    /**
     * 
     */
    kind: ResourceBodyOneOfKindEnum;
    /**
     * 
     */
    title: string;
    /**
     * 
     */
    root: string;
    /**
     * 
     */
    extensions: Array<string>;
    /**
     * 
     */
    maxBytes: number;
    /**
     * 
     */
    policy: Policy;
}


/**
 * @export
 */
export const ResourceBodyOneOfKindEnum = {
    Directory: 'directory',
} as const;
export type ResourceBodyOneOfKindEnum = typeof ResourceBodyOneOfKindEnum[keyof typeof ResourceBodyOneOfKindEnum];

/**
 * 
 * @export
 * @interface ResourceBodyOneOf1
 */
export interface ResourceBodyOneOf1 {
    /**
     * 
     */
    kind: ResourceBodyOneOf1KindEnum;
    /**
     * 
     */
    title: string;
    /**
     * 
     */
    baseUrl: string;
    /**
     * 
     */
    allowedOrigins: Array<string>;
    /**
     * 
     */
    maxBytes: number;
    /**
     * 
     */
    policy: Policy;
}


/**
 * @export
 */
export const ResourceBodyOneOf1KindEnum = {
    Http: 'http',
} as const;
export type ResourceBodyOneOf1KindEnum = typeof ResourceBodyOneOf1KindEnum[keyof typeof ResourceBodyOneOf1KindEnum];

/**
 * 
 * @export
 * @interface ResourceBodyOneOf2
 */
export interface ResourceBodyOneOf2 {
    /**
     * 
     */
    kind: ResourceBodyOneOf2KindEnum;
    /**
     * 
     */
    title: string;
    /**
     * 
     */
    connectorId: string;
    /**
     * 
     */
    allowedUris: Array<string>;
    /**
     * 
     */
    maxBytes: number;
    /**
     * 
     */
    policy: Policy;
}


/**
 * @export
 */
export const ResourceBodyOneOf2KindEnum = {
    Mcp: 'mcp',
} as const;
export type ResourceBodyOneOf2KindEnum = typeof ResourceBodyOneOf2KindEnum[keyof typeof ResourceBodyOneOf2KindEnum];

/**
 * 
 * @export
 * @interface ResourcesReadRequest
 */
export interface ResourcesReadRequest {
    /**
     * 
     */
    target: string;
}
/**
 * 
 * @export
 * @interface RoleProfile
 */
export interface RoleProfile {
    /**
     * 
     */
    modelResourceId: string;
    /**
     * 
     */
    temperature: number;
    /**
     * 
     */
    maxOutputTokens: number;
    /**
     * 
     */
    reasoning?: string;
}
/**
 * 
 * @export
 * @interface RoleProfileInput
 */
export interface RoleProfileInput {
    /**
     * 
     */
    modelResourceId: string;
    /**
     * 
     */
    temperature?: number;
    /**
     * 
     */
    maxOutputTokens?: number;
    /**
     * 
     */
    reasoning?: string;
}
/**
 * 
 * @export
 * @interface SeedApply
 */
export interface SeedApply {
    /**
     * 
     */
    id: string;
    /**
     * 
     */
    formed: TimePoint;
    /**
     * 
     */
    selectedIndexes?: Array<number>;
    /**
     * 
     */
    changes?: Array<KnowledgeChange>;
}
/**
 * 
 * @export
 * @interface SeedApplyInput
 */
export interface SeedApplyInput {
    /**
     * 
     */
    id: string;
    /**
     * 
     */
    formed: TimePointInput;
    /**
     * 
     */
    selectedIndexes?: Array<number>;
    /**
     * 
     */
    changes?: Array<KnowledgeChangeInput>;
}
/**
 * 
 * @export
 * @interface SeedsPreviewRequest
 */
export interface SeedsPreviewRequest {
    /**
     * 
     */
    refs: Array<RefInput>;
    /**
     * 
     */
    instructions?: string;
}
/**
 * 
 * @export
 * @interface Selector
 */
export interface Selector {
    /**
     * 
     */
    kind: SelectorKindEnum;
    /**
     * 
     */
    start?: number;
    /**
     * 
     */
    end?: number;
    /**
     * 
     */
    page?: number;
    /**
     * 
     */
    partId?: string;
    /**
     * 
     */
    nodeId?: string;
    /**
     * 
     */
    x?: number;
    /**
     * 
     */
    y?: number;
    /**
     * 
     */
    width?: number;
    /**
     * 
     */
    height?: number;
    /**
     * 
     */
    within?: Selector;
}


/**
 * @export
 */
export const SelectorKindEnum = {
    Text: 'text',
    Page: 'page',
    Image: 'image',
    Media: 'media',
    Part: 'part',
    Node: 'node',
} as const;
export type SelectorKindEnum = typeof SelectorKindEnum[keyof typeof SelectorKindEnum];

/**
 * 
 * @export
 * @interface SelectorInput
 */
export interface SelectorInput {
    /**
     * 
     */
    kind: SelectorInputKindEnum;
    /**
     * 
     */
    start?: number;
    /**
     * 
     */
    end?: number;
    /**
     * 
     */
    page?: number;
    /**
     * 
     */
    partId?: string;
    /**
     * 
     */
    nodeId?: string;
    /**
     * 
     */
    x?: number;
    /**
     * 
     */
    y?: number;
    /**
     * 
     */
    width?: number;
    /**
     * 
     */
    height?: number;
    /**
     * 
     */
    within?: SelectorInput;
}


/**
 * @export
 */
export const SelectorInputKindEnum = {
    Text: 'text',
    Page: 'page',
    Image: 'image',
    Media: 'media',
    Part: 'part',
    Node: 'node',
} as const;
export type SelectorInputKindEnum = typeof SelectorInputKindEnum[keyof typeof SelectorInputKindEnum];

/**
 * 
 * @export
 * @interface Self
 */
export interface Self {
    /**
     * 
     */
    content: string;
    /**
     * 
     */
    title?: string;
    /**
     * 
     */
    entityRefs: Array<Ref>;
    /**
     * 
     */
    conceptRefs: Array<Ref>;
    /**
     * 
     */
    tags: Array<string>;
    /**
     * 
     */
    kind: SelfKindEnum;
    /**
     * 
     */
    aspect: SelfAspectEnum;
    /**
     * 
     */
    context?: string;
    /**
     * 
     */
    stability: SelfStabilityEnum;
}


/**
 * @export
 */
export const SelfKindEnum = {
    Self: 'Self',
} as const;
export type SelfKindEnum = typeof SelfKindEnum[keyof typeof SelfKindEnum];

/**
 * @export
 */
export const SelfAspectEnum = {
    Identity: 'identity',
    Role: 'role',
    Preference: 'preference',
    Value: 'value',
    Expression: 'expression',
    Skill: 'skill',
    DomainKnowledge: 'domain_knowledge',
    Limitation: 'limitation',
    Narrative: 'narrative',
} as const;
export type SelfAspectEnum = typeof SelfAspectEnum[keyof typeof SelfAspectEnum];

/**
 * @export
 */
export const SelfStabilityEnum = {
    Developing: 'developing',
    Stable: 'stable',
} as const;
export type SelfStabilityEnum = typeof SelfStabilityEnum[keyof typeof SelfStabilityEnum];

/**
 * 
 * @export
 * @interface SelfInput
 */
export interface SelfInput {
    /**
     * 
     */
    content: string;
    /**
     * 
     */
    title?: string;
    /**
     * 
     */
    entityRefs?: Array<RefInput>;
    /**
     * 
     */
    conceptRefs?: Array<RefInput>;
    /**
     * 
     */
    tags?: Array<string>;
    /**
     * 
     */
    kind: SelfInputKindEnum;
    /**
     * 
     */
    aspect: SelfInputAspectEnum;
    /**
     * 
     */
    context?: string;
    /**
     * 
     */
    stability?: SelfInputStabilityEnum;
}


/**
 * @export
 */
export const SelfInputKindEnum = {
    Self: 'Self',
} as const;
export type SelfInputKindEnum = typeof SelfInputKindEnum[keyof typeof SelfInputKindEnum];

/**
 * @export
 */
export const SelfInputAspectEnum = {
    Identity: 'identity',
    Role: 'role',
    Preference: 'preference',
    Value: 'value',
    Expression: 'expression',
    Skill: 'skill',
    DomainKnowledge: 'domain_knowledge',
    Limitation: 'limitation',
    Narrative: 'narrative',
} as const;
export type SelfInputAspectEnum = typeof SelfInputAspectEnum[keyof typeof SelfInputAspectEnum];

/**
 * @export
 */
export const SelfInputStabilityEnum = {
    Developing: 'developing',
    Stable: 'stable',
} as const;
export type SelfInputStabilityEnum = typeof SelfInputStabilityEnum[keyof typeof SelfInputStabilityEnum];

/**
 * 
 * @export
 * @interface Settings
 */
export interface Settings {
    /**
     * 
     */
    roles: { [key: string]: RoleProfile; };
    /**
     * 
     */
    budget: SettingsBudget;
    /**
     * 
     */
    growth: SettingsGrowth;
    /**
     * 
     */
    retrieval: SettingsRetrieval;
}
/**
 * 
 * @export
 * @interface SettingsBudget
 */
export interface SettingsBudget {
    /**
     * 
     */
    dailyAmount: number | null;
    /**
     * 
     */
    dailyCalls: number;
    /**
     * 
     */
    perCallAmount: number | null;
}
/**
 * 
 * @export
 * @interface SettingsGrowth
 */
export interface SettingsGrowth {
    /**
     * 
     */
    enabled: boolean;
    /**
     * 
     */
    batchSize: number;
    /**
     * 
     */
    intervalMinutes: number;
}
/**
 * 
 * @export
 * @interface SettingsInput
 */
export interface SettingsInput {
    /**
     * 
     */
    roles?: { [key: string]: RoleProfileInput; };
    /**
     * 
     */
    budget?: SettingsInputBudget;
    /**
     * 
     */
    growth?: SettingsInputGrowth;
    /**
     * 
     */
    retrieval?: SettingsInputRetrieval;
}
/**
 * 
 * @export
 * @interface SettingsInputBudget
 */
export interface SettingsInputBudget {
    /**
     * 
     */
    dailyAmount?: number | null;
    /**
     * 
     */
    dailyCalls?: number;
    /**
     * 
     */
    perCallAmount?: number | null;
}
/**
 * 
 * @export
 * @interface SettingsInputGrowth
 */
export interface SettingsInputGrowth {
    /**
     * 
     */
    enabled?: boolean;
    /**
     * 
     */
    batchSize?: number;
    /**
     * 
     */
    intervalMinutes?: number;
}
/**
 * 
 * @export
 * @interface SettingsInputRetrieval
 */
export interface SettingsInputRetrieval {
    /**
     * 
     */
    limit?: number;
    /**
     * 
     */
    exploration?: number;
}
/**
 * 
 * @export
 * @interface SettingsRetrieval
 */
export interface SettingsRetrieval {
    /**
     * 
     */
    limit: number;
    /**
     * 
     */
    exploration: number;
}
/**
 * 
 * @export
 * @interface SettingsUpdateRequest
 */
export interface SettingsUpdateRequest {
    /**
     * 
     */
    scope: SettingsUpdateRequestScopeEnum;
    /**
     * 
     */
    role?: string;
    /**
     * 
     */
    values: { [key: string]: any; };
    /**
     * 
     */
    expectedRevision?: number;
}


/**
 * @export
 */
export const SettingsUpdateRequestScopeEnum = {
    Installation: 'installation',
    Subject: 'subject',
    Role: 'role',
} as const;
export type SettingsUpdateRequestScopeEnum = typeof SettingsUpdateRequestScopeEnum[keyof typeof SettingsUpdateRequestScopeEnum];

/**
 * 
 * @export
 * @interface Social
 */
export interface Social {
    /**
     * 
     */
    content: string;
    /**
     * 
     */
    title?: string;
    /**
     * 
     */
    entityRefs: Array<Ref>;
    /**
     * 
     */
    conceptRefs: Array<Ref>;
    /**
     * 
     */
    tags: Array<string>;
    /**
     * 
     */
    kind: SocialKindEnum;
    /**
     * 
     */
    aspect: SocialAspectEnum;
    /**
     * 
     */
    fromRef: Ref;
    /**
     * 
     */
    toRef?: Ref;
    /**
     * 
     */
    dimension?: string;
    /**
     * 
     */
    degree?: number;
    /**
     * 
     */
    domain?: string;
    /**
     * 
     */
    context?: string;
}


/**
 * @export
 */
export const SocialKindEnum = {
    Social: 'Social',
} as const;
export type SocialKindEnum = typeof SocialKindEnum[keyof typeof SocialKindEnum];

/**
 * @export
 */
export const SocialAspectEnum = {
    Person: 'person',
    Relation: 'relation',
    Group: 'group',
} as const;
export type SocialAspectEnum = typeof SocialAspectEnum[keyof typeof SocialAspectEnum];

/**
 * 
 * @export
 * @interface SocialInput
 */
export interface SocialInput {
    /**
     * 
     */
    content: string;
    /**
     * 
     */
    title?: string;
    /**
     * 
     */
    entityRefs?: Array<RefInput>;
    /**
     * 
     */
    conceptRefs?: Array<RefInput>;
    /**
     * 
     */
    tags?: Array<string>;
    /**
     * 
     */
    kind: SocialInputKindEnum;
    /**
     * 
     */
    aspect: SocialInputAspectEnum;
    /**
     * 
     */
    fromRef: RefInput;
    /**
     * 
     */
    toRef?: RefInput;
    /**
     * 
     */
    dimension?: string;
    /**
     * 
     */
    degree?: number;
    /**
     * 
     */
    domain?: string;
    /**
     * 
     */
    context?: string;
}


/**
 * @export
 */
export const SocialInputKindEnum = {
    Social: 'Social',
} as const;
export type SocialInputKindEnum = typeof SocialInputKindEnum[keyof typeof SocialInputKindEnum];

/**
 * @export
 */
export const SocialInputAspectEnum = {
    Person: 'person',
    Relation: 'relation',
    Group: 'group',
} as const;
export type SocialInputAspectEnum = typeof SocialInputAspectEnum[keyof typeof SocialInputAspectEnum];

/**
 * 
 * @export
 * @interface Subject
 */
export interface Subject {
    /**
     * 
     */
    id: string;
    /**
     * 
     */
    ref: Ref;
    /**
     * 
     */
    displayName: string;
    /**
     * 
     */
    capabilities: Array<KnowledgeKind>;
    /**
     * 
     */
    status: SubjectStatusEnum;
    /**
     * 
     */
    createdAt: string;
    /**
     * 
     */
    changeCursor: string;
    /**
     * 
     */
    branchedFrom: Ref | null;
}


/**
 * @export
 */
export const SubjectStatusEnum = {
    Active: 'active',
    Paused: 'paused',
    Stopped: 'stopped',
} as const;
export type SubjectStatusEnum = typeof SubjectStatusEnum[keyof typeof SubjectStatusEnum];

/**
 * 
 * @export
 * @interface SubjectCreate
 */
export interface SubjectCreate {
    /**
     * 
     */
    id?: string;
    /**
     * 
     */
    displayName: string;
    /**
     * 
     */
    capabilities: Array<KnowledgeKind>;
}
/**
 * 
 * @export
 * @interface SubjectCreateInput
 */
export interface SubjectCreateInput {
    /**
     * 
     */
    id?: string;
    /**
     * 
     */
    displayName: string;
    /**
     * 
     */
    capabilities?: Array<KnowledgeKindInput>;
}
/**
 * 
 * @export
 * @interface SubjectInput
 */
export interface SubjectInput {
    /**
     * 
     */
    id: string;
    /**
     * 
     */
    ref: RefInput;
    /**
     * 
     */
    displayName: string;
    /**
     * 
     */
    capabilities: Array<KnowledgeKindInput>;
    /**
     * 
     */
    status: SubjectInputStatusEnum;
    /**
     * 
     */
    createdAt: string;
    /**
     * 
     */
    changeCursor: string;
    /**
     * 
     */
    branchedFrom: RefInput | null;
}


/**
 * @export
 */
export const SubjectInputStatusEnum = {
    Active: 'active',
    Paused: 'paused',
    Stopped: 'stopped',
} as const;
export type SubjectInputStatusEnum = typeof SubjectInputStatusEnum[keyof typeof SubjectInputStatusEnum];

/**
 * 
 * @export
 * @interface SubjectsBindRequest
 */
export interface SubjectsBindRequest {
    /**
     * 
     */
    installationId: string;
    /**
     * 
     */
    subjectId: string;
    /**
     * 
     */
    connectionRef: string;
}
/**
 * 
 * @export
 * @interface SubjectsCapabilitiesRequest
 */
export interface SubjectsCapabilitiesRequest {
    /**
     * 
     */
    capabilities: Array<KnowledgeKindInput>;
}
/**
 * 
 * @export
 * @interface TimePoint
 */
export interface TimePoint {
    /**
     * 
     */
    clockId: string;
    /**
     * 
     */
    at: string;
}
/**
 * 
 * @export
 * @interface TimePointInput
 */
export interface TimePointInput {
    /**
     * 
     */
    clockId: string;
    /**
     * 
     */
    at: string;
}
/**
 * 
 * @export
 * @interface TimeRange
 */
export interface TimeRange {
    /**
     * 
     */
    start?: TimePoint;
    /**
     * 
     */
    end?: TimePoint;
}
/**
 * 
 * @export
 * @interface TimeRangeInput
 */
export interface TimeRangeInput {
    /**
     * 
     */
    start?: TimePointInput;
    /**
     * 
     */
    end?: TimePointInput;
}
/**
 * 
 * @export
 * @interface WorkContext
 */
export interface WorkContext {
    /**
     * 
     */
    goal: string;
    /**
     * 
     */
    openQuestions: Array<string>;
    /**
     * 
     */
    progress: string;
    /**
     * 
     */
    currentRefs: Array<Ref>;
    /**
     * 
     */
    temporaryNotes: Array<string>;
    /**
     * 
     */
    resumeCue: string;
    /**
     * 
     */
    participantRefs: Array<Ref>;
    /**
     * 
     */
    lifecycle: WorkContextLifecycleEnum;
    /**
     * 
     */
    relatedRefs: Array<Ref>;
    /**
     * 
     */
    scope: Consumer;
    /**
     * 
     */
    id: string;
    /**
     * 
     */
    subjectId: string;
    /**
     * 
     */
    revision: number;
    /**
     * 
     */
    ref: Ref;
    /**
     * 
     */
    recorded: string;
}


/**
 * @export
 */
export const WorkContextLifecycleEnum = {
    Active: 'active',
    Paused: 'paused',
    Completed: 'completed',
} as const;
export type WorkContextLifecycleEnum = typeof WorkContextLifecycleEnum[keyof typeof WorkContextLifecycleEnum];

/**
 * 
 * @export
 * @interface WorkContextBody
 */
export interface WorkContextBody {
    /**
     * 
     */
    goal: string;
    /**
     * 
     */
    openQuestions: Array<string>;
    /**
     * 
     */
    progress: string;
    /**
     * 
     */
    currentRefs: Array<Ref>;
    /**
     * 
     */
    temporaryNotes: Array<string>;
    /**
     * 
     */
    resumeCue: string;
    /**
     * 
     */
    participantRefs: Array<Ref>;
    /**
     * 
     */
    lifecycle: WorkContextBodyLifecycleEnum;
    /**
     * 
     */
    relatedRefs: Array<Ref>;
}


/**
 * @export
 */
export const WorkContextBodyLifecycleEnum = {
    Active: 'active',
    Paused: 'paused',
    Completed: 'completed',
} as const;
export type WorkContextBodyLifecycleEnum = typeof WorkContextBodyLifecycleEnum[keyof typeof WorkContextBodyLifecycleEnum];

/**
 * 
 * @export
 * @interface WorkContextBodyInput
 */
export interface WorkContextBodyInput {
    /**
     * 
     */
    goal: string;
    /**
     * 
     */
    openQuestions?: Array<string>;
    /**
     * 
     */
    progress?: string;
    /**
     * 
     */
    currentRefs?: Array<RefInput>;
    /**
     * 
     */
    temporaryNotes?: Array<string>;
    /**
     * 
     */
    resumeCue?: string;
    /**
     * 
     */
    participantRefs?: Array<RefInput>;
    /**
     * 
     */
    lifecycle?: WorkContextBodyInputLifecycleEnum;
    /**
     * 
     */
    relatedRefs?: Array<RefInput>;
}


/**
 * @export
 */
export const WorkContextBodyInputLifecycleEnum = {
    Active: 'active',
    Paused: 'paused',
    Completed: 'completed',
} as const;
export type WorkContextBodyInputLifecycleEnum = typeof WorkContextBodyInputLifecycleEnum[keyof typeof WorkContextBodyInputLifecycleEnum];

/**
 * 
 * @export
 * @interface WorkContextInput
 */
export interface WorkContextInput {
    /**
     * 
     */
    goal: string;
    /**
     * 
     */
    openQuestions?: Array<string>;
    /**
     * 
     */
    progress?: string;
    /**
     * 
     */
    currentRefs?: Array<RefInput>;
    /**
     * 
     */
    temporaryNotes?: Array<string>;
    /**
     * 
     */
    resumeCue?: string;
    /**
     * 
     */
    participantRefs?: Array<RefInput>;
    /**
     * 
     */
    lifecycle?: WorkContextInputLifecycleEnum;
    /**
     * 
     */
    relatedRefs?: Array<RefInput>;
    /**
     * 
     */
    scope: ConsumerInput;
    /**
     * 
     */
    id: string;
    /**
     * 
     */
    subjectId: string;
    /**
     * 
     */
    revision: number;
    /**
     * 
     */
    ref: RefInput;
    /**
     * 
     */
    recorded: string;
}


/**
 * @export
 */
export const WorkContextInputLifecycleEnum = {
    Active: 'active',
    Paused: 'paused',
    Completed: 'completed',
} as const;
export type WorkContextInputLifecycleEnum = typeof WorkContextInputLifecycleEnum[keyof typeof WorkContextInputLifecycleEnum];


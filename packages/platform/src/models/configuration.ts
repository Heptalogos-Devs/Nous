// Graph-link validation adapted from Nous-Wave 412b062082afb75ccfae4c3f69fc606d3e3cc862
// apps/nous-core/src/model/configuration.ts and execution/binding.ts (Apache-2.0).
// Copyright 2026 Aravine Zhu; SPDX-License-Identifier: AGPL-3.0-only
import { randomInt } from 'node:crypto';
import type { CallContext, Ref } from '@nous-wave/contracts/common';
import type { SecretUse } from '@nous-wave/contracts/credentials';
import {
  type InvocationSnapshot,
  InvocationSnapshotSchema,
  ModelBudgetSchema,
  type ModelRegistration,
  ModelRegistrationSchema,
  PriceDefinitionSchema,
  ProviderDefinitionSchema,
  type ResourceRef,
  RolePolicySchema,
} from '@nous-wave/contracts/models';
import type {
  ConfigScope,
  ConfigSnapshot,
  JsonObject,
  SettingsRevision,
} from '@nous-wave/contracts/settings';
import { LRUCache } from 'lru-cache';
import { z } from 'zod';
import { problem } from '../errors.js';
import { digest, immutable, parse, pointer } from '../settings/catalog.js';
import type { Settings, SettingsModule } from '../settings/index.js';
import type {
  ConfigurationRead,
  ModelSettingsDefinitions,
  ModelsOptions,
  NativeHostContext,
  PreparedModel,
  PrepareModelOptions,
  RoleDescriptor,
} from './types.js';

export const modelNamespaces = {
  providers: 'platform.providers',
  resources: 'platform.models',
  roles: 'platform.roles',
  prices: 'platform.modelPrices',
  budgets: 'platform.modelBudgets',
  limits: 'platform.modelLimits',
} as const;
const ProvidersSchema = z
  .strictObject({
    providers: z.record(z.string().min(1), ProviderDefinitionSchema),
  })
  .superRefine((value, ctx) => {
    for (const [key, item] of Object.entries(value.providers))
      if (key !== item.id)
        ctx.addIssue({
          code: 'custom',
          path: ['providers', key],
          message: 'Provider目录键必须等于ID',
        });
  });
const ResourcesSchema = z
  .strictObject({
    resources: z.record(z.string().min(1), ModelRegistrationSchema),
  })
  .superRefine((value, ctx) => {
    for (const [key, item] of Object.entries(value.resources))
      if (key !== item.definition.id)
        ctx.addIssue({
          code: 'custom',
          path: ['resources', key],
          message: '模型目录键必须等于ID',
        });
  });
const RolesSchema = z.strictObject({
  roles: z.record(z.string().min(1), RolePolicySchema),
});
const PricesSchema = z.strictObject({
  prices: z.record(z.string().min(1), PriceDefinitionSchema),
});
const BudgetsSchema = z.strictObject({
  budgets: z.record(z.string().min(1), ModelBudgetSchema),
});
const definitionRevision = 'models-v1';
export function modelSettingsModules(
  catalog: readonly RoleDescriptor[],
  adapters: ModelsOptions['providerAdapters'],
): ModelSettingsDefinitions {
  const ids = new Set<string>();
  const defaults: Record<string, RoleDescriptor['defaultPolicy']> = {};
  for (const role of catalog) {
    if (ids.has(role.id) || role.kind !== role.defaultPolicy.kind)
      throw problem(
        'models.invalid_role_catalog',
        'validation',
        '模型职责目录重复或类型不一致',
      );
    ids.add(role.id);
    defaults[role.id] = parse(RolePolicySchema, role.defaultPolicy);
  }
  const adapterMap = new Map(adapters.map((adapter) => [adapter.id, adapter]));
  if (adapterMap.size !== adapters.length)
    throw problem(
      'models.invalid_adapter_catalog',
      'validation',
      'Provider适配器ID重复',
    );
  const module = (
    key: keyof typeof modelNamespaces,
    schema: z.ZodType,
    initial: JsonObject,
    path: string,
    scopes: ConfigScope['kind'][],
  ): SettingsModule => ({
    id: modelNamespaces[key],
    definitionRevision,
    owner: 'models',
    schema: schema as z.ZodType<JsonObject>,
    defaults: initial,
    fields: {
      [path]: {
        labelKey: `models.${key}`,
        descriptionKey: `models.${key}.description`,
        scopes,
        visibility: 'advanced',
        effect: 'next_call',
        consumerIds: ['models.factory'],
      },
    },
    async validate(value) {
      if (key === 'providers')
        for (const provider of Object.values(
          parse(ProvidersSchema, value).providers,
        )) {
          const adapter = adapterMap.get(provider.adapter);
          if (!adapter)
            throw problem(
              'models.adapter_not_found',
              'validation',
              'Provider适配器未注册',
            );
          const options = parse(adapter.optionsSchema, provider.options);
          if (digest(options) !== digest(provider.options))
            throw problem(
              'models.invalid_adapter_options',
              'validation',
              'Provider配置含隐式转换或未声明参数',
            );
        }
      if (key === 'roles')
        for (const [id, policy] of Object.entries(
          parse(RolesSchema, value).roles,
        )) {
          const role = catalog.find((role) => role.id === id);
          if (!role || role.kind !== policy.kind)
            throw problem(
              'models.invalid_role',
              'validation',
              '职责未注册或模型类型不一致',
            );
        }
    },
  });
  const limits: SettingsModule = {
    id: modelNamespaces.limits,
    definitionRevision,
    owner: 'models',
    schema: ModelBudgetSchema as z.ZodType<JsonObject>,
    defaults: {
      maxCalls: 100,
      concurrentCalls: 4,
      totalAmount: null,
      dailyAmount: null,
      perCallAmount: null,
      window: { kind: 'rolling', durationMs: 86400000 },
      timeZone: 'UTC',
    },
    fields: Object.fromEntries(
      Object.keys(ModelBudgetSchema.shape).map((key) => [
        `/${key}`,
        {
          labelKey: `models.limits.${key}`,
          descriptionKey: `models.limits.${key}.description`,
          scopes: ['installation', 'subject', 'role'],
          visibility: 'advanced',
          effect: 'next_call',
          consumerIds: ['models.factory'],
          ...(['window', 'timeZone'].includes(key)
            ? {}
            : { restriction: 'upper_bound' }),
        },
      ]),
    ) as SettingsModule['fields'],
  };
  return {
    ...modelNamespaces,
    modules: [
      module('providers', ProvidersSchema, { providers: {} }, '/providers', [
        'installation',
      ]),
      module('resources', ResourcesSchema, { resources: {} }, '/resources', [
        'installation',
      ]),
      module(
        'roles',
        RolesSchema,
        { roles: defaults } as JsonObject,
        '/roles',
        ['installation', 'subject', 'role'],
      ),
      module('prices', PricesSchema, { prices: {} }, '/prices', [
        'installation',
      ]),
      module('budgets', BudgetsSchema, { budgets: {} }, '/budgets', [
        'installation',
      ]),
      limits,
    ],
  };
}
const currentRevision = (snapshot: ConfigSnapshot, namespace: string) =>
  snapshot.revisions.find(
    (ref) => ref.namespace === namespace && ref.scope.kind === 'installation',
  )?.revision ?? 0;
export function createModelConfiguration(options: ModelsOptions) {
  const installation = {
    kind: 'installation' as const,
    installationId: options.installationId,
  };
  const definitions = modelSettingsModules(
    options.roleCatalog,
    options.providerAdapters,
  );
  const modules = new Map(definitions.modules.map((item) => [item.id, item]));
  const groups = new LRUCache<string, ReturnType<Settings['registerConsumer']>>(
    { max: 128, dispose: (value) => value.close() },
  );
  const limitsReaders = new LRUCache<
    string,
    ReturnType<Settings['registerConsumer']>
  >({ max: 256, dispose: (value) => value.close() });
  const installed = Object.entries(modelNamespaces).filter(
    ([key]) => key !== 'roles' && key !== 'limits',
  );
  const installationReaders = new Map(
    installed.map(([, namespace]) => [
      namespace,
      options.settings.registerConsumer({
        namespace,
        scope: installation,
        consumerId: 'models.factory',
        processBootId: options.processBootId,
      }),
    ]),
  );
  const roles = new Map(options.roleCatalog.map((role) => [role.id, role]));
  function modelRef(id: string, revision?: number): Ref {
    return {
      product: options.product,
      installationId: options.installationId,
      kind: 'ModelResource',
      id,
      ...(revision ? { revision } : {}),
    };
  }
  function providerRef(id: string, revision?: number): Ref {
    return {
      product: options.product,
      installationId: options.installationId,
      kind: 'Provider',
      id,
      ...(revision ? { revision } : {}),
    };
  }
  async function load(
    ctx: CallContext,
    roleId?: string,
    selection?: ConfigScope,
  ): Promise<ConfigurationRead> {
    const scope: ConfigScope =
      selection ??
      (ctx.subjectId
        ? roleId
          ? { kind: 'role', subjectId: ctx.subjectId, roleId }
          : { kind: 'subject', subjectId: ctx.subjectId }
        : installation);
    const key = digest(scope);
    let consumer = groups.get(key);
    if (!consumer) {
      consumer = options.settings.registerConsumer({
        processBootId: options.processBootId,
        consumerId: 'models.factory',
        scope,
        namespace: modelNamespaces.roles,
      });
      groups.set(key, consumer);
    }
    const ancestors: ConfigScope[] = [
      installation,
      ...(scope.kind !== 'installation'
        ? [{ kind: 'subject' as const, subjectId: scope.subjectId }]
        : []),
      ...(scope.kind === 'role' ? [scope] : []),
    ];
    const limitConsumers = ancestors.map((scope) => {
      const key = digest(scope);
      let consumer = limitsReaders.get(key);
      if (!consumer) {
        consumer = options.settings.registerConsumer({
          processBootId: options.processBootId,
          consumerId: 'models.factory',
          scope,
          namespace: modelNamespaces.limits,
        });
        limitsReaders.set(key, consumer);
      }
      return consumer;
    });
    const selected = [
      installationReaders.get(modelNamespaces.providers)!,
      installationReaders.get(modelNamespaces.resources)!,
      consumer,
      installationReaders.get(modelNamespaces.prices)!,
      installationReaders.get(modelNamespaces.budgets)!,
      ...limitConsumers,
    ];
    const values = await options.settings.snapshotConsumers(selected);
    selected.forEach((consumer, index) => {
      consumer.reportApplied(values[index]);
    });
    return {
      providers: values[0],
      resources: values[1],
      roles: values[2],
      prices: values[3],
      budgets: values[4],
      limits: ancestors.map((scope, index) => ({
        scope,
        snapshot: values[index + 5],
      })),
      scope,
    };
  }
  async function resourceSnapshot(
    ref: ResourceRef,
    current: ConfigurationRead,
  ) {
    const currentResources = parse(
      ResourcesSchema,
      current.resources.values,
    ).resources;
    const enabled = currentResources[ref.id];
    if (!enabled || !enabled.definition.enabled)
      throw problem(
        'models.resource_unavailable',
        'unavailable',
        '模型资源当前未启用',
      );
    const snapshot =
      ref.revision ===
      currentRevision(current.resources, modelNamespaces.resources)
        ? current.resources
        : await installationReaders
            .get(modelNamespaces.resources)!
            .snapshotRevision(ref.revision);
    const selected = parse(ResourcesSchema, snapshot.values).resources[ref.id];
    if (!selected)
      throw problem(
        'models.resource_not_found',
        'not_found',
        '指定修订中没有该模型资源',
      );
    return { registration: selected, snapshot };
  }
  async function atResource(
    ref: ResourceRef,
    current: ConfigurationRead,
  ): Promise<ModelRegistration> {
    return (await resourceSnapshot(ref, current)).registration;
  }
  async function definition(
    ctx: CallContext,
    registration: ModelRegistration,
    local?: NativeHostContext,
  ) {
    if (registration.managedBy === 'sdk') return registration;
    if (!options.nativeBindings)
      throw problem(
        'models.native_unavailable',
        'unavailable',
        '原生模型配置owner不可用',
      );
    const selected = registration.definition,
      binding = {
        ...(await options.nativeBindings.read(ctx, selected, local)),
        id: selected.id,
      };
    if (
      binding.runtimeId !== selected.runtimeId ||
      binding.nativeResourceId !== selected.nativeResourceId
    )
      throw problem(
        'models.native_identity_mismatch',
        'conflict',
        '原生模型绑定与实际资源不一致',
      );
    return {
      managedBy: 'native' as const,
      id: selected.id,
      binding,
      enabled: selected.enabled,
      ...(selected.pricingRef ? { pricingRef: selected.pricingRef } : {}),
    };
  }
  async function prepare(
    ctx: CallContext,
    roleId: string,
    input: PrepareModelOptions = {},
  ): Promise<PreparedModel> {
    const role = roles.get(roleId);
    if (!role)
      throw problem('models.role_not_found', 'not_found', '模型职责未注册');
    const config = await load(ctx, roleId),
      policy = parse(RolesSchema, config.roles.values).roles[roleId];
    if (!policy?.enabled || policy.kind !== role.kind)
      throw problem(
        'models.role_unavailable',
        'unavailable',
        '模型职责未配置或未启用',
      );
    const providers = parse(ProvidersSchema, config.providers.values).providers;
    const candidates: InvocationSnapshot[] = [];
    for (const ref of input.resource ? [input.resource] : policy.resources) {
      if (
        input.resource &&
        !policy.resources.some(
          (item) => item.id === ref.id && item.revision === ref.revision,
        )
      )
        throw problem(
          'models.resource_not_allowed',
          'forbidden',
          '指定模型不在职责候选范围',
        );
      try {
        await options.authorize(
          ctx,
          'models.invoke',
          modelRef(ref.id, ref.revision),
        );
        const pinned = await resourceSnapshot(ref, config),
          registration = pinned.registration,
          selected = await definition(ctx, registration, input.nativeHost);
        const resource =
          selected.managedBy === 'sdk' ? selected.definition : selected.binding;
        if (
          resource.kind !== role.kind ||
          role.requiredCapabilities.some(
            (capability) => !resource.capabilities.includes(capability),
          )
        )
          continue;
        const provider =
          selected.managedBy === 'sdk'
            ? providers[selected.definition.providerId]
            : undefined;
        if (
          selected.managedBy === 'sdk' &&
          (!provider?.enabled ||
            !options.providerAdapters.some(
              (adapter) => adapter.id === provider.adapter,
            ))
        )
          continue;
        const pricingId =
          selected.managedBy === 'sdk'
            ? selected.definition.pricingRef
            : selected.pricingRef;
        const snapshots = {
          providers: config.providers,
          resources: pinned.snapshot,
          roles: config.roles,
          prices: config.prices,
          budgets: config.budgets,
          limits: config.limits.at(-1)!.snapshot,
        };
        const revisions = Object.values(snapshots).flatMap(
            (snapshot) => snapshot.revisions,
          ),
          definitionRevisions = Object.fromEntries(
            Object.values(modelNamespaces).map((namespace, index) => [
              namespace,
              Object.values(snapshots)[index].definitionRevision,
            ]),
          );
        const price = pricingId
          ? parse(PricesSchema, config.prices.values).prices[pricingId]
          : undefined;
        if (pricingId && !price)
          throw problem(
            'models.price_not_found',
            'unavailable',
            '模型绑定的价目不存在',
          );
        candidates.push(
          parse(InvocationSnapshotSchema, {
            config: {
              revisions,
              definitionRevisions,
              digest: digest(snapshots),
            },
            roleId,
            requestedResource: ref,
            actualResource: ref,
            actualProvider:
              provider?.id ??
              (resource as { actualProvider: string }).actualProvider,
            actualModelId:
              selected.managedBy === 'sdk'
                ? selected.definition.modelId
                : selected.binding.actualModelId,
            definition: selected,
            ...(provider ? { provider } : {}),
            policy,
            inference: policy.inference,
            ...(price
              ? {
                  pricing: {
                    ...price,
                    revision: currentRevision(
                      config.prices,
                      modelNamespaces.prices,
                    ),
                  },
                }
              : {}),
          }),
        );
      } catch (error) {
        if (input.resource) throw error;
        if (error && typeof error === 'object' && 'problem' in error) {
          const category = (error.problem as { category?: string }).category;
          if (
            ['forbidden', 'not_found', 'unavailable'].includes(category ?? '')
          )
            continue;
        }
        throw error;
      }
    }
    if (!candidates.length)
      throw problem(
        'models.no_resource',
        'unavailable',
        '没有满足职责、能力与当前授权的模型资源',
      );
    let index = 0;
    if (policy.selection === 'random') index = randomInt(candidates.length);
    if (policy.selection === 'balance') {
      const counts = await options.db
        .selectFrom('modelInvocations')
        .select(['resourceId'])
        .where('status', 'in', ['prepared', 'dispatched', 'unknown'])
        .where(
          'resourceId',
          'in',
          candidates.map((item) => item.actualResource.id),
        )
        .execute();
      const load = (id: string) =>
        counts.filter((item) => item.resourceId === id).length;
      candidates.forEach((candidate, current) => {
        if (
          load(candidate.actualResource.id) <
          load(candidates[index].actualResource.id)
        )
          index = current;
      });
    }
    let snapshot = candidates[index];
    const budget = await options.budgetScopes.resolve(ctx, {
      roleId,
      allocation: input.budgetAllocation,
    });
    if (budget.allocation)
      snapshot = { ...snapshot, budgetAllocation: budget.allocation };
    let instructions: string | undefined;
    if (policy.prompt) {
      const prompt = await options.prompts.snapshot(ctx, {
        binding: policy.prompt,
        scope: ctx.subjectId
          ? { kind: 'subject', subjectId: ctx.subjectId }
          : installation,
      });
      const rendered = await options.prompts.render(ctx, {
        snapshot: prompt.ref,
        params: input.promptParams ?? {},
      });
      instructions = rendered.text;
      snapshot = { ...snapshot, prompt: prompt.ref };
    }
    const ownBudget = policy.budgetPolicyRef
      ? parse(BudgetsSchema, config.budgets.values).budgets[
          policy.budgetPolicyRef
        ]
      : undefined;
    if (policy.budgetPolicyRef && !ownBudget)
      throw problem(
        'models.budget_not_found',
        'unavailable',
        '职责绑定的预算策略不存在',
      );
    return immutable({
      snapshot,
      instructions,
      constraints: [
        ...budget.constraints,
        ...config.limits.map(({ scope, snapshot }) => ({
          key: `model-limits:${options.product}:${options.installationId}:${digest(scope)}`,
          budget: parse(ModelBudgetSchema, snapshot.values),
        })),
        ...(ownBudget
          ? [
              {
                key: `role:${ctx.subjectId ?? options.installationId}:${roleId}`,
                budget: ownBudget,
              },
            ]
          : []),
      ],
    });
  }
  async function assertCurrent(
    ctx: CallContext,
    snapshot: InvocationSnapshot,
    local?: NativeHostContext,
  ) {
    await options.authorize(
      ctx,
      'models.invoke',
      modelRef(snapshot.actualResource.id, snapshot.actualResource.revision),
    );
    const current = await load(ctx, snapshot.roleId);
    await atResource(snapshot.actualResource, current);
    const role = parse(RolesSchema, current.roles.values).roles[
      snapshot.roleId
    ];
    if (
      !role?.enabled ||
      role.kind !== snapshot.policy.kind ||
      !role.resources.some(
        (resource) =>
          resource.id === snapshot.actualResource.id &&
          resource.revision === snapshot.actualResource.revision,
      )
    )
      throw problem(
        'models.role_changed',
        'forbidden',
        '职责当前未启用或已撤销该资源候选',
      );
    if (snapshot.provider) {
      const provider = parse(ProvidersSchema, current.providers.values)
        .providers[snapshot.provider.id];
      if (
        !provider?.enabled ||
        provider.adapter !== snapshot.provider.adapter ||
        provider.credentialRef !== snapshot.provider.credentialRef
      )
        throw problem(
          'models.provider_changed',
          'conflict',
          'Provider绑定或可用性已变化，请重新准备调用',
        );
    }
    if (snapshot.definition.managedBy === 'native') {
      const currentBinding = await definition(
        ctx,
        await atResource(snapshot.actualResource, current),
        local,
      );
      if (
        currentBinding.managedBy !== 'native' ||
        digest(currentBinding.binding) !== digest(snapshot.definition.binding)
      )
        throw problem(
          'models.native_changed',
          'conflict',
          '原生模型配置修订已变化，请重新准备调用',
        );
    }
    return current;
  }
  function management(kind: 'providers' | 'resources') {
    const namespace = modelNamespaces[kind],
      key = kind;
    const schema =
      kind === 'providers' ? ProviderDefinitionSchema : ModelRegistrationSchema;
    const target = kind === 'providers' ? providerRef : modelRef;
    const parseDictionary = (value: unknown) =>
      kind === 'providers'
        ? parse(ProvidersSchema, value).providers
        : parse(ResourcesSchema, value).resources;
    return {
      async list(ctx: CallContext) {
        await options.authorize(
          ctx,
          `${kind === 'providers' ? 'providers' : 'models'}.read`,
        );
        const snapshot = await installationReaders.get(namespace)!.snapshot();
        const revision = currentRevision(snapshot, namespace);
        return Object.values(parseDictionary(snapshot.values)).map(
          (definition) => ({
            ref: {
              id:
                'managedBy' in definition
                  ? definition.definition.id
                  : definition.id,
              revision,
            },
            definition,
          }),
        );
      },
      async read(ctx: CallContext, input: { id: string; revision?: number }) {
        await options.authorize(
          ctx,
          `${kind === 'providers' ? 'providers' : 'models'}.read`,
          target(input.id, input.revision),
        );
        const snapshot = input.revision
          ? await installationReaders
              .get(namespace)!
              .snapshotRevision(input.revision)
          : await installationReaders.get(namespace)!.snapshot();
        const value = parseDictionary(snapshot.values)[input.id];
        if (!value)
          throw problem(
            'models.resource_not_found',
            'not_found',
            '资源定义不存在',
          );
        return {
          ref: {
            id: input.id,
            revision: input.revision ?? currentRevision(snapshot, namespace),
          },
          definition: value,
          snapshot,
        };
      },
      async create(
        ctx: CallContext,
        input: { definition: unknown; expectedRevision: number },
      ) {
        const value = parse(schema as z.ZodType, input.definition) as
          | z.infer<typeof ProviderDefinitionSchema>
          | ModelRegistration;
        const id = 'managedBy' in value ? value.definition.id : value.id;
        await options.authorize(
          ctx,
          `${kind === 'providers' ? 'providers' : 'models'}.manage`,
          target(id),
        );
        const snapshot = await installationReaders.get(namespace)!.snapshot(),
          dictionary = parseDictionary(snapshot.values);
        if (Object.hasOwn(dictionary, id))
          throw problem('models.resource_exists', 'conflict', '资源定义已存在');
        return options.settings.update(ctx, {
          namespace,
          scope: installation,
          expectedRevision: input.expectedRevision,
          patch: [
            {
              op: 'add',
              path: pointer([key]),
              value: { ...dictionary, [id]: value } as JsonObject,
            },
          ],
        });
      },
      async update(
        ctx: CallContext,
        input: { id: string; definition: unknown; expectedRevision: number },
      ) {
        const value = parse(schema as z.ZodType, input.definition) as
          | z.infer<typeof ProviderDefinitionSchema>
          | ModelRegistration;
        const id = 'managedBy' in value ? value.definition.id : value.id;
        if (id !== input.id)
          throw problem(
            'models.identity_change',
            'validation',
            '修改不能改变资源ID',
          );
        await options.authorize(
          ctx,
          `${kind === 'providers' ? 'providers' : 'models'}.manage`,
          target(id),
        );
        return options.settings.update(ctx, {
          namespace,
          scope: installation,
          expectedRevision: input.expectedRevision,
          patch: [
            {
              op: 'replace',
              path: pointer([key, id]),
              value: value as JsonObject,
            },
          ],
        });
      },
      async delete(
        ctx: CallContext,
        input: { id: string; expectedRevision: number },
      ) {
        await options.authorize(
          ctx,
          `${kind === 'providers' ? 'providers' : 'models'}.manage`,
          target(input.id),
        );
        return options.settings.update(ctx, {
          namespace,
          scope: installation,
          expectedRevision: input.expectedRevision,
          patch: [{ op: 'remove', path: pointer([key, input.id]) }],
        });
      },
    };
  }
  return {
    definitions,
    load,
    prepare,
    assertCurrent,
    modelRef,
    providers: management('providers'),
    resources: management('resources'),
    async nativeResource(ctx: CallContext, roleId: string, id: string) {
      const config = await load(ctx, roleId),
        policy = parse(RolesSchema, config.roles.values).roles[roleId],
        ref = policy?.resources.find((resource) => resource.id === id);
      if (!ref)
        throw problem(
          'models.native_resource_not_allowed',
          'forbidden',
          '实际原生模型不在职责候选范围',
        );
      return ref;
    },
    async credentialTarget(
      ctx: CallContext,
      input: SecretUse & { useRef: Ref },
    ) {
      const { resourceRef, useRef } = input;
      if (
        resourceRef.kind !== 'Provider' ||
        useRef.kind !== 'ModelResource' ||
        resourceRef.product !== options.product ||
        useRef.product !== options.product ||
        resourceRef.installationId !== options.installationId ||
        useRef.installationId !== options.installationId ||
        !useRef.revision
      )
        throw problem(
          'models.credential_binding',
          'forbidden',
          '模型凭据必须经实际Provider与固定模型关系使用',
        );
      const config = await load(ctx),
        selected = await atResource(
          { id: useRef.id, revision: useRef.revision },
          config,
        ),
        providers = parse(ProvidersSchema, config.providers.values).providers;
      if (
        selected.managedBy !== 'sdk' ||
        selected.definition.providerId !== resourceRef.id ||
        !providers[resourceRef.id]?.enabled ||
        providers[resourceRef.id]?.credentialRef !== input.ref
      )
        throw problem(
          'models.credential_binding',
          'forbidden',
          '模型当前未绑定该Provider凭据',
        );
      return { operation: 'models.invoke', target: useRef };
    },
    async networkSelection(ctx: CallContext, ref: Ref) {
      if (
        ref.product !== options.product ||
        ref.installationId !== options.installationId ||
        ref.kind !== 'ModelResource' ||
        ref.subjectId ||
        ref.selector
      )
        throw problem(
          'models.resource_scope',
          'forbidden',
          '网络调用需要本安装正式模型资源引用',
        );
      const config = await load(ctx),
        revision =
          ref.revision ??
          currentRevision(config.resources, modelNamespaces.resources),
        fixed = modelRef(ref.id, revision);
      await options.authorize(ctx, 'models.invoke', fixed);
      const selected = await atResource({ id: ref.id, revision }, config);
      if (selected.managedBy !== 'sdk')
        throw problem(
          'models.native_network_owned',
          'unavailable',
          '原生模型网络由真实运行时owner提供',
        );
      const provider = parse(ProvidersSchema, config.providers.values)
        .providers[selected.definition.providerId];
      if (!provider?.enabled)
        throw problem(
          'models.provider_unavailable',
          'unavailable',
          '模型当前Provider未启用',
        );
      const adapter = options.providerAdapters.find(
        (adapter) => adapter.id === provider.adapter,
      );
      if (!adapter)
        throw problem(
          'models.adapter_not_found',
          'unavailable',
          '模型Provider适配器不可用',
        );
      return {
        resource: adapter.networkScope({ provider, resourceRef: fixed }),
        scope: ctx.subjectId
          ? { kind: 'subject' as const, subjectId: ctx.subjectId }
          : installation,
      };
    },
    async budgetConstraints(ctx: CallContext, snapshot: InvocationSnapshot) {
      const current = await load(ctx, snapshot.roleId),
        own = snapshot.policy.budgetPolicyRef
          ? parse(BudgetsSchema, current.budgets.values).budgets[
              snapshot.policy.budgetPolicyRef
            ]
          : undefined;
      return [
        ...current.limits.map(({ scope, snapshot }) => ({
          key: `model-limits:${options.product}:${options.installationId}:${digest(scope)}`,
          budget: parse(ModelBudgetSchema, snapshot.values),
        })),
        ...(own
          ? [
              {
                key: `role:${ctx.subjectId ?? options.installationId}:${snapshot.roleId}`,
                budget: own,
              },
            ]
          : []),
      ];
    },
    async status() {
      const [providerSnapshot, resourceSnapshot] =
        await options.settings.snapshotConsumers([
          installationReaders.get(modelNamespaces.providers)!,
          installationReaders.get(modelNamespaces.resources)!,
        ]);
      const providers = Object.values(
          parse(ProvidersSchema, providerSnapshot.values).providers,
        ),
        resources = Object.values(
          parse(ResourcesSchema, resourceSnapshot.values).resources,
        );
      return {
        state: 'not_probed' as const,
        providers: {
          configured: providers.length,
          enabled: providers.filter((provider) => provider.enabled).length,
        },
        resources: {
          configured: resources.length,
          enabled: resources.filter((resource) => resource.definition.enabled)
            .length,
        },
      };
    },
    async describeRoles(ctx: CallContext, scope?: ConfigScope) {
      await options.authorize(
        ctx,
        'roles.read',
        scope && scope.kind !== 'installation'
          ? {
              product: options.product,
              installationId: options.installationId,
              subjectId: scope.subjectId,
              kind: 'Subject',
              id: scope.subjectId,
            }
          : undefined,
      );
      const config = await load(ctx, undefined, scope);
      const policies = parse(RolesSchema, config.roles.values).roles;
      return options.roleCatalog.map((role) => ({
        ...role,
        policy: policies[role.id],
        snapshot: config.roles,
      }));
    },
    close() {
      groups.clear();
      limitsReaders.clear();
      for (const consumer of installationReaders.values()) consumer.close();
    },
  };
}
export type ModelConfiguration = ReturnType<typeof createModelConfiguration>;

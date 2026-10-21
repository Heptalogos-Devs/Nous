import type {
  AuthenticationOptions,
  OperationPolicy,
} from '@nous-wave/platform/credentials';
/** Exact operation catalog consumed by current domain owners. */
export const subjectOperations = [
  'subjects.read',
  'materials.capture',
  'materials.publish',
  'materials.read',
  'encounters.accept',
  'encounters.read',
  'knowledge.create',
  'knowledge.revise',
  'knowledge.read',
  'contexts.read',
  'contexts.updateWork',
  'contexts.prepare',
  'cognition.feedback',
  'growth.run',
  'growth.inspect',
  'growth.apply',
  'growth.resolve',
  'seeds.preview',
  'seeds.apply',
  'maintenance.run',
  'maintenance.inspect',
  'maintenance.pause',
  'episodes.organize',
  'journals.write',
  'concepts.merge',
  'concepts.split',
  'motivation.evaluate',
  'resources.read',
  'resources.register',
  'models.invoke',
  'models.read',
  'costs.read',
  'operations.read',
  'operations.cancel',
  'operations.resume',
  'settings.read',
  'prompts.snapshot',
  'prompts.render',
  'delegations.create',
  'clock.read',
  'clock.capture',
  'clock.references.read',
] as const;
export const managementOperations = [
  ...subjectOperations,
  'subjects.create',
  'subjects.bind',
  'subjects.provision',
  'installation.manage',
  'policy.update',
  'data.manage',
  'data.export',
  'data.import',
  'data.purge',
  'models.manage',
  'secrets.manage',
  'models.test',
  'providers.read',
  'providers.manage',
  'roles.read',
  'models.invocations.read',
  'models.reconcile',
  'network.manage',
  'network.read',
  'network.probe',
  'native.connect',
  'native.record',
  'prompts.describe',
  'prompts.list',
  'prompts.read',
  'prompts.save',
  'prompts.activate',
  'prompts.reset',
  'prompts.preview',
  'prompts.history',
  'prompts.export',
  'prompts.watch',
  'settings.export',
  'settings.import',
  'settings.deleteScopes',
  'settings.update',
  'settings.reset',
  'settings.restore',
  'settings.history',
  'settings.describe',
  'settings.applied',
  'delegations.select',
  'delegations.read',
  'delegations.update',
  'delegations.revoke',
  'host.status',
  'host.drain',
  'host.stop',
  'openapi.read',
] as const;
export function operationPolicy(
  operation: string,
): OperationPolicy | undefined {
  if (!(managementOperations as readonly string[]).includes(operation))
    return undefined;
  return {
    allowedPlanes:
      ['delegations.select', 'delegations.read', 'delegations.revoke'].includes(
        operation,
      ) || (subjectOperations as readonly string[]).includes(operation)
        ? ['system', 'subject']
        : ['system'],
    targetRequired: operation === 'models.invoke',
  };
}
/** Default local management key; issuance is limited to an authenticated user. */
export function managementScope(
  installationId: string,
): import('@nous-wave/platform/credentials').DelegationScope {
  return {
    plane: 'system',
    operations: [...managementOperations],
    subjectIds: [],
    consumer: { kind: 'maintenance', host: 'nous', id: installationId },
    purposes: ['*'],
    audiences: ['*'],
    targets: [],
    inputRefs: [],
    expiresAt: {
      domainId: 'real',
      at: new Date(Date.now() + 86400000).toISOString().replace('Z', '000Z'),
    },
    mayDelegate: true,
  };
}

export function issuancePolicy(
  installationId: string,
): NonNullable<AuthenticationOptions['issuancePolicy']> {
  return ({ issuer, requested, requestedPrincipalKind }) => {
    if (requested.plane !== 'subject' || requested.subjectIds.length !== 1)
      return false;
    if (
      !requested.operations.every(
        (op) =>
          (subjectOperations as readonly string[]).includes(op) ||
          [
            'delegations.select',
            'delegations.read',
            'delegations.revoke',
          ].includes(op),
      )
    )
      return false;
    if (
      issuer.scope &&
      (!requested.purposes.every(
        (value) =>
          issuer.scope!.purposes.includes('*') ||
          issuer.scope!.purposes.includes(value),
      ) ||
        !requested.audiences.every(
          (value) =>
            issuer.scope!.audiences.includes('*') ||
            issuer.scope!.audiences.includes(value),
        ))
    )
      return false;
    if (
      issuer.consumer.kind === 'maintenance' &&
      issuer.consumer.host === 'nous'
    ) {
      return requested.consumer.kind === 'subject'
        ? ['subject', 'service'].includes(requestedPrincipalKind) &&
            requested.consumer.id === requested.subjectIds[0] &&
            ['nous', installationId].includes(requested.consumer.host)
        : (requested.consumer.kind === 'worker' &&
            ['worker', 'service'].includes(requestedPrincipalKind)) ||
            (requested.consumer.kind === 'author' &&
              ['author', 'service'].includes(requestedPrincipalKind));
    }
    if (
      issuer.actor.kind === 'service' &&
      issuer.consumer.kind === 'worker' &&
      issuer.scope?.mayDelegate &&
      issuer.scope.operations.includes('delegations.select')
    ) {
      const task = requested.taskRef,
        native = requested.nativeBinding;
      return (
        requestedPrincipalKind === 'worker' &&
        requested.consumer.kind === 'worker' &&
        !requested.mayDelegate &&
        Boolean(
          task &&
            native &&
            task.product === 'hept' &&
            task.installationId === issuer.consumer.host &&
            task.subjectId === requested.subjectIds[0] &&
            task.kind === 'task' &&
            !task.selector &&
            task.revision === undefined &&
            issuer.consumer.id === requested.subjectIds[0] &&
            issuer.scope.subjectIds.includes(requested.subjectIds[0]) &&
            requested.consumer.id === task.id &&
            requested.consumer.host === native.runtimeId &&
            native.rootTaskId === task.id,
        ) &&
        requested.operations.every(
          (op) =>
            issuer.scope!.operations.includes(op) &&
            !['delegations.create', 'delegations.select'].includes(op),
        )
      );
    }
    return (
      issuer.consumer.kind === 'subject' &&
      issuer.consumer.id === requested.subjectIds[0] &&
      issuer.scope?.subjectIds.includes(requested.subjectIds[0]) === true &&
      requestedPrincipalKind === 'worker' &&
      requested.consumer.kind === 'worker' &&
      requested.consumer.host === installationId &&
      requested.consumer.id.startsWith(`${requested.subjectIds[0]}:`) &&
      !requested.mayDelegate &&
      !requested.operations.includes('delegations.create')
    );
  };
}

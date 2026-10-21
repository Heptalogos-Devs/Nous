import { readFile } from 'node:fs/promises';
import type { DefaultApi } from '@nous-wave/client';
import type { Command } from 'commander';

export function addManagementCommands(options: {
  cli: Command;
  withClient<T>(use: (client: DefaultApi) => Promise<T>): Promise<T>;
  show(value: unknown): void;
}) {
  const { cli, withClient, show } = options;
  const input = async (file: string) =>
    JSON.parse(await readFile(file, 'utf8'));
  const providers = cli
    .command('providers')
    .description('模型提供方与连接配置');
  providers
    .command('list')
    .action(async () => show(await withClient((c) => c.providersList())));
  providers
    .command('read <id>')
    .option('--revision <number>', '历史修订', Number)
    .action(async (id, o) =>
      show(
        await withClient((c) => c.providersRead({ id, revision: o.revision })),
      ),
    );
  providers
    .command('create <file>')
    .action(async (file) =>
      show(
        await withClient(async (c) =>
          c.providersCreate({ providersCreateRequest: await input(file) }),
        ),
      ),
    );
  providers
    .command('update <id> <file>')
    .action(async (id, file) =>
      show(
        await withClient(async (c) =>
          c.providersUpdate({ id, providersCreateRequest: await input(file) }),
        ),
      ),
    );
  providers.command('delete <id> <revision>').action(async (id, revision) =>
    show(
      await withClient((c) =>
        c.providersDelete({
          id,
          providersDeleteRequest: { expectedRevision: Number(revision) },
        }),
      ),
    ),
  );

  const models = cli.command('models').description('具体模型、能力与真实调用');
  models
    .command('list')
    .action(async () => show(await withClient((c) => c.modelsList())));
  models
    .command('read <id>')
    .option('--revision <number>', '历史修订', Number)
    .action(async (id, o) =>
      show(await withClient((c) => c.modelsRead({ id, revision: o.revision }))),
    );
  models
    .command('create <file>')
    .action(async (file) =>
      show(
        await withClient(async (c) =>
          c.modelsCreate({ modelsCreateRequest: await input(file) }),
        ),
      ),
    );
  models
    .command('update <id> <file>')
    .action(async (id, file) =>
      show(
        await withClient(async (c) =>
          c.modelsUpdate({ id, modelsCreateRequest: await input(file) }),
        ),
      ),
    );
  models.command('delete <id> <revision>').action(async (id, revision) =>
    show(
      await withClient((c) =>
        c.modelsDelete({
          id,
          providersDeleteRequest: { expectedRevision: Number(revision) },
        }),
      ),
    ),
  );
  models
    .command('test <file>')
    .description('按正式角色配置调用一次模型；使用当前权限和预算')
    .action(async (file) =>
      show(
        await withClient(async (c) =>
          c.modelsTest({ modelTestBodyInput: await input(file) }),
        ),
      ),
    );
  const roles = cli
    .command('roles')
    .description('角色要求及有效绑定；通过settings修改稀疏覆盖');
  roles
    .command('list [file]')
    .action(async (file) =>
      show(
        await withClient(async (c) =>
          c.modelsRoles(file ? await input(file) : {}),
        ),
      ),
    );
  const invocations = models
    .command('invocations')
    .description('真实调用、恢复状态与输入输出引用');
  invocations
    .command('list [file]')
    .action(async (file) =>
      show(
        await withClient(async (c) =>
          c.modelsInvocationsList(file ? await input(file) : {}),
        ),
      ),
    );
  invocations
    .command('read <id>')
    .option('--subject <id>', '限定主体；安装级调用留空')
    .action(async (id, o) =>
      show(
        await withClient((c) =>
          c.modelsInvocationsRead({ id, subjectId: o.subject }),
        ),
      ),
    );
  invocations
    .command('reconcile <file>')
    .action(async (file) =>
      show(
        await withClient(async (c) =>
          c.modelsReconcile({ modelsReconcileRequest: await input(file) }),
        ),
      ),
    );
  cli
    .command('costs [file]')
    .description('按主体、角色或状态读取实际usage与计价事实')
    .action(async (file) =>
      show(
        await withClient(async (c) =>
          c.costsRead(file ? await input(file) : {}),
        ),
      ),
    );

  const prompts = cli
    .command('prompts')
    .description('Prompt资源、覆盖、预览和修订');
  prompts
    .command('catalog [file]')
    .action(async (file) =>
      show(
        await withClient(async (c) =>
          c.promptsDescribe(file ? await input(file) : {}),
        ),
      ),
    );
  prompts
    .command('list <file>')
    .action(async (file) =>
      show(await withClient(async (c) => c.promptsList(await input(file)))),
    );
  prompts
    .command('read <file>')
    .action(async (file) =>
      show(await withClient(async (c) => c.promptsRead(await input(file)))),
    );
  prompts
    .command('history <file>')
    .action(async (file) =>
      show(await withClient(async (c) => c.promptsHistory(await input(file)))),
    );
  prompts
    .command('save <file>')
    .action(async (file) =>
      show(
        await withClient(async (c) =>
          c.promptsSave({ promptSaveRequestInput: await input(file) }),
        ),
      ),
    );
  prompts
    .command('activate <file>')
    .action(async (file) =>
      show(
        await withClient(async (c) =>
          c.promptsActivate({ promptActivateRequestInput: await input(file) }),
        ),
      ),
    );
  prompts
    .command('reset <file>')
    .action(async (file) =>
      show(
        await withClient(async (c) =>
          c.promptsReset({ promptResetRequestInput: await input(file) }),
        ),
      ),
    );
  prompts
    .command('preview <file>')
    .action(async (file) =>
      show(
        await withClient(async (c) =>
          c.promptsPreview({ promptPreviewRequestInput: await input(file) }),
        ),
      ),
    );
  prompts
    .command('snapshot <file>')
    .action(async (file) =>
      show(
        await withClient(async (c) =>
          c.promptsSnapshot({ promptSnapshotRequestInput: await input(file) }),
        ),
      ),
    );
  prompts
    .command('render <file>')
    .action(async (file) =>
      show(
        await withClient(async (c) =>
          c.promptsRender({ promptRenderRequestInput: await input(file) }),
        ),
      ),
    );

  const network = cli
    .command('network')
    .description('有效连接配置与实际网络探测');
  network
    .command('inspect [file]')
    .action(async (file) =>
      show(
        await withClient(async (c) =>
          c.networkInspect(file ? await input(file) : {}),
        ),
      ),
    );
  network
    .command('probe <file>')
    .action(async (file) =>
      show(
        await withClient(async (c) =>
          c.networkProbe({ networkProbeRequest: await input(file) }),
        ),
      ),
    );
  const time = cli.command('time').description('通用时间来源与幂等时间捕获');
  time
    .command('read [file]')
    .action(async (file) =>
      show(
        await withClient(async (c) =>
          c.timeRead({ timeReadRequest: file ? await input(file) : {} }),
        ),
      ),
    );
  time
    .command('capture <file>')
    .action(async (file) =>
      show(
        await withClient(async (c) =>
          c.timeCapture({ timeCaptureBodyInput: await input(file) }),
        ),
      ),
    );
  time
    .command('receipt <subject> <id>')
    .action(async (subjectId, id) =>
      show(await withClient((c) => c.timeReadCapture({ subjectId, id }))),
    );

  const settings = cli
    .command('settings')
    .description('模块配置、修订与实际生效状态');
  settings
    .command('schema')
    .action(async () => show(await withClient((c) => c.settingsDescribe({}))));
  settings
    .command('read <file>')
    .action(async (file) =>
      show(await withClient(async (c) => c.settingsRead(await input(file)))),
    );
  settings
    .command('history <file>')
    .action(async (file) =>
      show(await withClient(async (c) => c.settingsHistory(await input(file)))),
    );
  settings
    .command('applied <file>')
    .action(async (file) =>
      show(await withClient(async (c) => c.settingsApplied(await input(file)))),
    );
  settings
    .command('update <file>')
    .action(async (file) =>
      show(
        await withClient(async (c) =>
          c.settingsUpdate({ settingsChangeInput: await input(file) }),
        ),
      ),
    );
  settings
    .command('reset <file>')
    .action(async (file) =>
      show(
        await withClient(async (c) =>
          c.settingsReset({ settingsResetInput: await input(file) }),
        ),
      ),
    );
  settings
    .command('restore <file>')
    .action(async (file) =>
      show(
        await withClient(async (c) =>
          c.settingsRestore({ settingsRestoreInput: await input(file) }),
        ),
      ),
    );
}

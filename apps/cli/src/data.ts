import { openAsBlob, createWriteStream } from 'node:fs';
import { readFile, rm } from 'node:fs/promises';
import { Readable } from 'node:stream';
import type { ReadableStream as NodeReadableStream } from 'node:stream/web';
import { pipeline } from 'node:stream/promises';
import { DataApi, type Configuration } from '@nous-wave/client';
import { RefSchema } from '@nous-wave/contracts/common';
import { ImportPlanSchema, PurgePlanSchema } from '@nous-wave/contracts/data';
import type { Command } from 'commander';
import { problem } from '@nous-wave/platform/errors';

export function addDataCommands(options: {
  cli: Command;
  withClient<A, T>(
    Api: new (configuration: Configuration) => A,
    use: (client: A) => Promise<T>,
  ): Promise<T>;
  show(value: unknown): void;
}) {
  const { cli, withClient, show } = options;
  const json = async (path: string) => JSON.parse(await readFile(path, 'utf8'));
  const reference = async (path: string) => RefSchema.parse(await json(path));
  const data = cli
    .command('data')
    .description('主体归档、恢复、清除与操作回执');
  data
    .command('export <subject>')
    .description('提交暂停主体的归档任务')
    .action(async (subject) =>
      show(
        await withClient(DataApi, (client) =>
          client.dataExport({
            subjectId: subject,
            dataExportRequest: { mode: 'restore' },
          }),
        ),
      ),
    );
  data
    .command('receipt <operation>')
    .description('读取正式数据结果；使用提交返回的operationId')
    .action(async (id) =>
      show(
        await withClient(DataApi, (client) => client.dataReadReceipt({ id })),
      ),
    );
  data
    .command('upload <bundle>')
    .description('流式上传待验证的归档文件')
    .action(async (path) =>
      show(
        await withClient(DataApi, async (client) =>
          client.dataUploadImportArtifact({ body: await openAsBlob(path) }),
        ),
      ),
    );
  data
    .command('artifact <ref-file>')
    .description('读取JSON Ref文件所指的归档信息')
    .action(async (path) =>
      show(
        await withClient(DataApi, async (client) =>
          client.dataReadArtifact({ artifactRef: await reference(path) }),
        ),
      ),
    );
  data
    .command('download <ref-file> <output>')
    .description('流式下载归档；输出文件须不存在')
    .action(async (path, output) => {
      await withClient(DataApi, async (client) => {
        const response = await client.dataDownloadArtifactRaw({
          artifactRef: await reference(path),
        });
        if (!response.raw.body)
          throw problem(
            'data.response_empty',
            'unavailable',
            '归档响应没有字节流',
          );
        const target = createWriteStream(output, { flags: 'wx', mode: 0o600 });
        let created = false;
        target.once('open', () => {
          created = true;
        });
        try {
          await pipeline(Readable.fromWeb(response.raw.body as NodeReadableStream<Uint8Array>), target);
        } catch (error) {
          if (created) await rm(output, { force: true });
          throw error;
        }
      });
      show({ path: output });
    });
  data
    .command('preview <ref-file>')
    .description('验证上传归档并建立恢复计划')
    .action(async (path) =>
      show(
        await withClient(DataApi, async (client) =>
          client.dataImportPreview({
            dataArtifactReadInput: { artifactRef: await reference(path) },
          }),
        ),
      ),
    );
  data
    .command('import <subject> <plan-file>')
    .description('按已验证计划恢复原主体，保持暂停')
    .action(async (subject, path) => {
      const plan = ImportPlanSchema.parse(await json(path));
      show(
        await withClient(DataApi, (client) =>
          client.dataImport({
            subjectId: subject,
            dataImportInput: {
              planId: plan.id,
              expectedTargets: plan.expectedTargets,
            },
          }),
        ),
      );
    });
  data
    .command('plan-purge <subject>')
    .description('建立主体清除计划')
    .action(async (subject) =>
      show(
        await withClient(DataApi, (client) =>
          client.dataPlanPurge({ subjectId: subject, body: {} }),
        ),
      ),
    );
  data
    .command('purge <subject> <plan-file>')
    .description('按明确计划清除暂停主体及其授权')
    .action(async (subject, path) => {
      const plan = PurgePlanSchema.parse(await json(path));
      show(
        await withClient(DataApi, (client) =>
          client.dataPurge({
            subjectId: subject,
            dataImportInput: {
              planId: plan.id,
              expectedTargets: plan.expectedTargets,
            },
          }),
        ),
      );
    });
}

import { spawnSync } from 'node:child_process';
import { mkdir, mkdtemp, rename, rm, writeFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import writeFileAtomic from 'write-file-atomic';

const image = 'openapitools/openapi-generator-cli:v7.26.0';

function run(command: string, args: string[], cwd?: string) {
  const result = spawnSync(command, args, {
    cwd,
    encoding: 'utf8',
    maxBuffer: 16 * 1024 * 1024,
  });
  if (result.error || result.status !== 0) {
    process.stderr.write((result.stderr ?? result.stdout ?? '').slice(-8000));
    throw new Error(
      `OpenAPI tool failed: ${command} (${result.status ?? 'unavailable'})`,
    );
  }
}

/** Explicit developer operation. The official generator and its templates remain the implementation. */
export async function generateOpenApiClient(options: {
  document: unknown;
  schemaPath: string;
  clientPath: string;
  temporaryRoot: string;
}): Promise<void> {
  await mkdir(options.temporaryRoot, { recursive: true });
  const stage = await mkdtemp(
    join(resolve(options.temporaryRoot), 'openapi-client-'),
  );
  const output = join(stage, 'output');
  const templates = join(stage, 'templates');
  const previous = join(stage, 'previous');
  const input = join(stage, 'input.json');
  const client = resolve(options.clientPath);
  const schemaPath = resolve(options.schemaPath);
  const document = `${JSON.stringify(options.document, null, 2)}\n`;
  const user = process.getuid
    ? ['--user', `${process.getuid()}:${process.getgid!()}`]
    : [];
  let saved = false;
  let installed = false;
  try {
    await Promise.all([
      mkdir(output),
      mkdir(templates),
      writeFile(input, document),
    ]);
    run('docker', [
      'run',
      '--rm',
      '--memory=512m',
      '--cpus=1',
      ...user,
      '-v',
      `${templates}:/templates`,
      image,
      'author',
      'template',
      '-g',
      'typescript-fetch',
      '-o',
      '/templates',
    ]);
    run(
      'git',
      [
        'apply',
        fileURLToPath(
          new URL('../../resources/openapi/binary-body.patch', import.meta.url),
        ),
      ],
      templates,
    );
    run('docker', [
      'run',
      '--rm',
      '--memory=1g',
      '--cpus=2',
      ...user,
      '-v',
      `${input}:/spec/input.json:ro`,
      '-v',
      `${templates}:/templates:ro`,
      '-v',
      `${output}:/generated`,
      image,
      'generate',
      '-i',
      '/spec/input.json',
      '-g',
      'typescript-fetch',
      '-t',
      '/templates',
      '-o',
      '/generated',
      '--global-property',
      'apis,models,supportingFiles=runtime.ts:index.ts,apiDocs=false,modelDocs=false,apiTests=false,modelTests=false',
      '--reserved-words-mappings',
      'native=native',
      '--additional-properties',
      'importFileExtension=.js,dateLibrary=string,withoutRuntimeChecks=true,hideGenerationTimestamp=true,prefixParameterInterfaces=true',
    ]);
    await mkdir(dirname(client), { recursive: true });
    await mkdir(dirname(schemaPath), { recursive: true });
    try {
      await rename(client, previous);
      saved = true;
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error;
    }
    await rename(output, client);
    installed = true;
    await writeFileAtomic(schemaPath, document, { encoding: 'utf8' });
  } catch (error) {
    if (installed) await rm(client, { recursive: true, force: true });
    if (saved) await rename(previous, client);
    throw error;
  } finally {
    await rm(stage, { recursive: true, force: true });
  }
}

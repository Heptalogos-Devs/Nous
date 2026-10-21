import { createHash } from 'node:crypto';
import { mkdtemp, mkdir, readFile, writeFile, rm } from 'node:fs/promises';
import { join, resolve, isAbsolute } from 'node:path';
import { execa } from 'execa';
import { create, extract } from 'tar';

/** Ordinary npm tarballs with a closed set of versioned GitHub dependencies. */
export async function packRelease(options: {
  repository: string;
  tag: string;
  revision: string;
  packages: readonly string[];
  output: string;
  temporaryRoot: string;
  pnpmCli: string;
}) {
  if (
    !/^[\w.-]+\/[\w.-]+$/.test(options.repository) ||
    !isAbsolute(options.pnpmCli)
  )
    throw new TypeError(
      'Use a GitHub repository and the absolute npm_execpath supplied by pnpm',
    );
  const temporaryRoot = resolve(options.temporaryRoot);
  const output = resolve(options.output);
  await Promise.all([
    mkdir(temporaryRoot, { recursive: true }),
    mkdir(output, { recursive: true }),
  ]);
  const stage = await mkdtemp(join(temporaryRoot, 'release-'));
  try {
    const packages = await Promise.all(
      options.packages.map(async (directory) => {
        const manifest = JSON.parse(
          await readFile(join(directory, 'package.json'), 'utf8'),
        );
        const filename = `${manifest.name.replace(/^@/, '').replace('/', '-')}-${manifest.version}.tgz`;
        const url = `https://github.com/${options.repository}/releases/download/${encodeURIComponent(options.tag)}/${filename}`;
        return {
          directory: resolve(directory),
          name: manifest.name as string,
          version: manifest.version as string,
          filename,
          url,
        };
      }),
    );
    const urls = Object.fromEntries(
      packages.map((item) => [item.name, item.url]),
    );
    const artifacts = [];
    for (const item of packages) {
      const unpack = join(stage, item.name.replaceAll('/', '-'));
      await mkdir(unpack);
      await execa(options.pnpmCli, ['pack', '--pack-destination', unpack], {
        cwd: item.directory,
      });
      await extract({
        file: join(unpack, item.filename),
        cwd: unpack,
        strict: true,
      });
      const manifestPath = join(unpack, 'package', 'package.json');
      const manifest = JSON.parse(await readFile(manifestPath, 'utf8'));
      for (const field of [
        'dependencies',
        'optionalDependencies',
        'devDependencies',
      ])
        for (const name of Object.keys(manifest[field] ?? {}))
          if (urls[name]) manifest[field][name] = urls[name];
      await writeFile(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`);
      const target = join(output, item.filename);
      await create(
        {
          file: target,
          cwd: unpack,
          gzip: true,
          portable: true,
          mtime: new Date(0),
        },
        ['package'],
      );
      const bytes = await readFile(target);
      artifacts.push({
        name: item.name,
        version: item.version,
        filename: item.filename,
        url: item.url,
        bytes: bytes.length,
        sha256: createHash('sha256').update(bytes).digest('hex'),
      });
    }
    const release = {
      repository: options.repository,
      tag: options.tag,
      revision: options.revision,
      artifacts,
    };
    await writeFile(
      join(output, 'release.json'),
      `${JSON.stringify(release, null, 2)}\n`,
    );
    return release;
  } finally {
    await rm(stage, { recursive: true, force: true });
  }
}

import { readFile, writeFile, mkdir, access } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { createRequire } from 'node:module';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';

// CI tests the same ordinary tarballs before their versioned release URLs exist.
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const { values } = parseArgs({ options: {
  phase: { type: 'string' }, temporary: { type: 'string' }, hept: { type: 'string' },
  'runtime-source': { type: 'string' }, 'runtime-artifacts': { type: 'string' },
} });
if (!values.temporary || !values.hept || !['hept', 'install'].includes(values.phase))
  throw new Error('Supply --phase hept|install --temporary DIRECTORY --hept CHECKOUT');
const temporary = path.resolve(values.temporary), hept = path.resolve(values.hept);
const sdk = path.join(temporary, 'sdk');
const nousPackages = ['packages/contracts', 'packages/client', 'packages/platform',
  'integrations/openclaw', 'packages/nous', 'apps/server', 'apps/cli'];
const heptPackages = ['packages/contracts', 'packages/client', 'integrations/openclaw'];
async function localPackages(checkout, packages) {
  return Promise.all(packages.map(async directory => {
    const { name, version } = JSON.parse(await readFile(path.join(checkout, directory, 'package.json'), 'utf8'));
    const filename = `${name.replace(/^@/, '').replace('/', '-')}-${version}.tgz`;
    const file = path.join(sdk, filename), bytes = await readFile(file);
    return { name, version, filename, file, sha256: createHash('sha256').update(bytes).digest('hex') };
  }));
}
const nous = await localPackages(root, nousPackages);
const overrides = Object.fromEntries(nous.map(item => [item.name, `file:${item.file}`]));
if (!values['runtime-source'] || !values['runtime-artifacts'])
  throw new Error('Supply the official runtime source and package artifacts');
const runtimeManifest = JSON.parse(await readFile(path.resolve(values['runtime-source'], 'package.json'), 'utf8'));
const runtime = path.resolve(values['runtime-artifacts'], `openclaw-${runtimeManifest.version}.tgz`);
await access(runtime);
overrides.openclaw = `file:${runtime}`;
if (values.phase === 'hept') {
  const require = createRequire(import.meta.resolve('@nous-wave/platform/bootstrap'));
  const { parseYAML, stringifyYAML } = await import(require.resolve('confbox'));
  const file = path.join(hept, 'pnpm-workspace.yaml');
  const workspace = parseYAML(await readFile(file, 'utf8'));
  workspace.overrides = { ...workspace.overrides, ...overrides };
  const upstream = parseYAML(await readFile(path.resolve(values['runtime-source'], 'pnpm-workspace.yaml'), 'utf8'));
  workspace.allowBuilds = { ...upstream.allowBuilds, ...workspace.allowBuilds, openclaw: true };
  const manifestPath = path.join(hept, 'package.json');
  const manifest = JSON.parse(await readFile(manifestPath, 'utf8'));
  // The actual built SDK is an explicit test dependency for channel consumers.
  manifest.devDependencies = { ...manifest.devDependencies, openclaw: `file:${runtime}` };
  await writeFile(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`);
  await writeFile(file, stringifyYAML(workspace));
  await writeFile(path.join(temporary, 'nous-local-packages.json'), JSON.stringify(nous, null, 2));
} else {
  const heptArtifacts = await localPackages(hept, heptPackages);
  for (const item of heptArtifacts) overrides[item.name] = `file:${item.file}`;
  const install = path.join(temporary, 'installed');
  await mkdir(install, { recursive: true });
  await writeFile(path.join(install, 'package.json'), JSON.stringify({
    name: 'openclaw-runtime-smoke', private: true, type: 'module',
    dependencies: { openclaw: `file:${runtime}`, ...overrides }, overrides,
  }, null, 2));
  await writeFile(path.join(temporary, 'local-package-installation.json'), JSON.stringify({
    evidence: 'CI ordinary local tarball installation; versioned release publication is separate',
    runtime, packages: [...nous, ...heptArtifacts],
  }, null, 2));
}
console.log(JSON.stringify({ phase: values.phase, packageOverrides: Object.keys(overrides).length }));

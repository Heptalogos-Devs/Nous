import { spawn } from 'node:child_process';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const directory = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(directory, '../..');
const source = path.resolve(process.argv[2] ?? path.join(root, 'data/upstream/openclaw'));
const output = path.join(root, 'data/native/openclaw');
const provenance = JSON.parse(await readFile(path.join(directory, 'upstream/source.json'), 'utf8'));
const patch = path.join(directory, 'upstream', provenance.patch);
async function run(command, args, cwd = source, { quiet = false, memory } = {}) {
  if (memory) {
    args = ['--user', '--scope', '-p', `MemoryMax=${memory}`, '-p', 'MemorySwapMax=512M', '-p', 'CPUQuota=200%',
      'env', 'GOMEMLIMIT=1536MiB', 'GOMAXPROCS=2', 'RAYON_NUM_THREADS=2', 'TOKIO_WORKER_THREADS=2',
      'NODE_OPTIONS=--max-old-space-size=2048', 'OPENCLAW_TSDOWN_MAX_OLD_SPACE_MB=2048', command, ...args];
    command = 'systemd-run';
  }
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, { cwd, stdio: quiet ? 'ignore' : 'inherit', env: process.env });
    child.on('error', reject);
    child.on('exit', code => code === 0 ? resolve() : reject(new Error(`${command} exited ${code}`)));
  });
}
await mkdir(path.dirname(source), { recursive: true });
await writeFile(path.join(path.dirname(source), '.nomedia'), '');
try { await run('git', ['rev-parse', '--git-dir'], source, { quiet: true }); }
catch { await run('git', ['clone', '--depth', '1', '--branch', provenance.tag, provenance.repository, source], path.dirname(source)); }
await run('git', ['merge-base', '--is-ancestor', provenance.commit, 'HEAD']);
try { await run('git', ['apply', '--reverse', '--check', patch], source, { quiet: true }); }
catch { await run('git', ['apply', '--check', patch]); await run('git', ['apply', patch]); }
await mkdir(output, { recursive: true });
await run('corepack', ['pnpm', 'install', '--frozen-lockfile', '--network-concurrency=4', '--child-concurrency=1'], source, { memory: '3G' });
await run('node', ['--import', './scripts/tsx.mjs', 'scripts/build-all.mts', 'package'], source, { memory: '6G' });
await run('node', ['scripts/package-openclaw-for-docker.mjs', '--skip-build', '--allow-unreleased-changelog', '--output-dir', output], source, { memory: '3G' });

import { packRelease } from '@nous-wave/platform/devtools/release';
import { execa } from 'execa';
import { resolve } from 'node:path';

const pnpmCli = process.env.npm_execpath;
if (!pnpmCli)
  throw new Error(
    'Run corepack pnpm pack:release after building the workspace',
  );
const { stdout: revision } = await execa('git', ['rev-parse', 'HEAD']);
const release = await packRelease({
  repository: 'Heptalogos-Devs/Nous-Wave',
  tag: 'v0.2.0-infrastructure.1',
  revision,
  pnpmCli,
  packages: [
    'packages/contracts',
    'packages/client',
    'packages/platform',
    'integrations/openclaw',
    'packages/nous',
    'apps/server',
    'apps/cli',
  ].map((path) => resolve(path)),
  temporaryRoot: resolve('../temp/releases/staging'),
  output: resolve('../temp/releases/v0.2.0-infrastructure.1'),
});
process.stdout.write(
  `${release.artifacts.length} packages: ${release.repository} ${release.tag}\n`,
);

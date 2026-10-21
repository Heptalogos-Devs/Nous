import { dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { safeProblem } from '@nous-wave/platform/errors';
import { runHost } from './host.js';

try {await runHost({programRoot:dirname(fileURLToPath(import.meta.resolve('@nous-wave/nous/package.json')))});}
catch(error) {process.stderr.write(JSON.stringify(safeProblem(error,'host.start_failed'))+'\n');process.exitCode=1;}

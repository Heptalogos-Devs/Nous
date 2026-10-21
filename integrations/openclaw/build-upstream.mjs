import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdir, readFile, writeFile, lstat } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';

const directory = path.dirname(fileURLToPath(import.meta.url));
const workspace = path.resolve(directory, '../../..');
const temporary = path.join(workspace, 'temp/spec-research/native-release');
const { values } = parseArgs({ options: {
  source: { type: 'string', default: path.join(temporary, 'source') },
  output: { type: 'string', default: path.join(temporary, 'package') },
  cache: { type: 'string', default: path.join(temporary, 'cache') },
  profile: { type: 'string', default: 'package' },
  'systemd-scope': { type: 'boolean', default: false },
  'skip-install': { type: 'boolean', default: false },
  'skip-build': { type: 'boolean', default: false },
  'build-only': { type: 'boolean', default: false },
  help: { type: 'boolean', default: false },
} });
if (values.help) {
  console.log('Build the pinned OpenClaw fork through its official build and package owners.\nOptions: --source PATH --output PATH --cache PATH --profile package|ciArtifacts|gatewayWatch|strictSmoke\n         --systemd-scope --skip-install --skip-build --build-only');
  process.exit(0);
}
if (process.versions.node.split('.')[0] !== '24') throw new Error('Use an explicitly installed Node.js 24 runtime');
if (values['systemd-scope'] && process.platform !== 'linux') throw new Error('--systemd-scope requires Linux and a user systemd instance');
const source = path.resolve(values.source), output = path.resolve(values.output), cache = path.resolve(values.cache);
const provenance = JSON.parse(await readFile(path.join(directory, 'upstream/source.json'), 'utf8'));
const patch = path.join(directory, 'upstream', provenance.patch);
if (createHash('sha256').update(await readFile(patch)).digest('hex') !== provenance.patchSha256) throw new Error('OpenClaw patch digest differs from source.json');
await Promise.all([mkdir(path.dirname(source), {recursive:true}), mkdir(output, {recursive:true}), mkdir(cache, {recursive:true}), mkdir(path.join(cache,'tmp'), {recursive:true})]);
await writeFile(path.join(path.dirname(source), '.nomedia'), '');
const environment = {
  ...process.env,
  BUILD_ALL_CACHE_ROOT: path.join(cache, 'build-all'),
  COREPACK_HOME: path.join(cache, 'corepack'),
  XDG_CACHE_HOME: path.join(cache, 'xdg'),
  npm_config_cache: path.join(cache, 'npm'),
  TMPDIR: path.join(cache, 'tmp'),
  GOMEMLIMIT: '1536MiB', GOMAXPROCS: '2', RAYON_NUM_THREADS: '2', TOKIO_WORKER_THREADS: '2',
};
async function run(command, args, {cwd=source, memory=3072, heap=2048, capture=false}={}) {
  const env = {...environment, NODE_OPTIONS:`--max-old-space-size=${heap}`, OPENCLAW_TSDOWN_MAX_OLD_SPACE_MB:String(heap)};
  if (values['systemd-scope']) {
    args = ['--user','--scope','-p',`MemoryMax=${memory}M`,'-p','MemorySwapMax=512M','-p','CPUQuota=200%',command,...args];
    command = 'systemd-run';
  }
  return new Promise((resolve,reject)=>{
    let stdout='';
    const child=spawn(command,args,{cwd,env,stdio:capture?['ignore','pipe','pipe']:'inherit'});
    if(capture){child.stdout.on('data',value=>{stdout+=value;});child.stderr.resume();}
    child.once('error',reject);
    child.once('exit',(code,signal)=>code===0?resolve(stdout.trim()):reject(new Error(`${command} exited ${code??signal}`)));
  });
}
let exists=true;
try {await lstat(source);} catch(error) {if(error.code==='ENOENT')exists=false;else throw error;}
if(!exists) await run('git',['clone','--depth','1','--branch',provenance.tag,provenance.repository,source],{cwd:path.dirname(source),memory:512,heap:256});
const head=await run('git',['rev-parse','HEAD'],{capture:true,memory:512,heap:256});
if(head!==provenance.commit && head!==provenance.modifiedCommit) throw new Error('The selected source is not the pinned base or adapted commit');
try {await run('git',['apply','--reverse','--check',patch],{capture:true,memory:512,heap:256});}
catch {await run('git',['apply','--check',patch],{memory:512,heap:256});await run('git',['apply',patch],{memory:512,heap:256});}
const actualPatch=await run('git',['diff','--binary',provenance.commit],{capture:true,memory:512,heap:256});
if(`${actualPatch}\n`!==await readFile(patch,'utf8')) throw new Error('The selected source has changes outside the pinned adaptation');
const installed=await lstat(path.join(source,'node_modules')).catch(error=>{if(error.code==='ENOENT')return null;throw error;});
if(installed?.isSymbolicLink()) throw new Error('OpenClaw declaration compilation requires dependencies physically inside the selected checkout; use an independent installation');
if(!values['skip-install']) await run('pnpm',['install','--frozen-lockfile','--network-concurrency=4','--child-concurrency=1','--store-dir',path.join(cache,'pnpm-store')]);
if(!values['skip-build']) await run('node',['--import','./scripts/tsx.mjs','scripts/build-all.mts',values.profile],{memory:6144,heap:4352});
if(!values['build-only']) await run('node',['scripts/package-openclaw-for-docker.mjs','--skip-build','--allow-unreleased-changelog','--output-dir',output]);
console.log(values['build-only']?source:output);

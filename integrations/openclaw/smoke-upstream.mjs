import { spawn } from 'node:child_process';
import { mkdir, mkdtemp, readFile, writeFile } from 'node:fs/promises';
import { randomBytes } from 'node:crypto';
import path from 'node:path';
import { parseArgs } from 'node:util';
import { setTimeout as delay } from 'node:timers/promises';
import { fileURLToPath } from 'node:url';

const workspace=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'../../..');
const {values}=parseArgs({options:{
 root:{type:'string'}, 'nous-plugin':{type:'string'}, 'hept-plugin':{type:'string'},
 'nous-installation-id':{type:'string'}, output:{type:'string',default:path.join(workspace,'temp/spec-research/native-release/smoke')},
 port:{type:'string',default:'18797'}, help:{type:'boolean',default:false},
}});
if(values.help){console.log('Start an installed OpenClaw package with the real plugins, verify health/authenticated routes, then stop.\n--root PACKAGE_ROOT --nous-plugin BUNDLED_DIR --hept-plugin BUNDLED_DIR --nous-installation-id ACTUAL_ID [--output TEMP_DIR] [--port 18797]');process.exit(0);}
if(!values.root||!values['nous-plugin']||!values['hept-plugin']||!values['nous-installation-id'])throw new Error('Supply the installed runtime, both bundled plugins, and an actual Nous bootstrap installation ID');
const port=Number(values.port);if(!Number.isInteger(port)||port<1||port>65535)throw new Error('Invalid Gateway port');
const runtime=path.resolve(values.root),nous=path.resolve(values['nous-plugin']),hept=path.resolve(values['hept-plugin']);
await Promise.all([readFile(path.join(runtime,'openclaw.mjs')),readFile(path.join(nous,'openclaw.plugin.json')),readFile(path.join(hept,'openclaw.plugin.json'))]);
await mkdir(values.output,{recursive:true,mode:0o700});
const root=await mkdtemp(path.join(path.resolve(values.output),'gateway-')),state=path.join(root,'state');
await mkdir(state,{mode:0o700});
const config={gateway:{mode:'local',bind:'loopback',port,auth:{mode:'token'},controlUi:{enabled:false}},agents:{defaults:{workspace:path.join(root,'workspace'),heartbeat:{every:'0m'}}},cron:{enabled:false},plugins:{allow:['nous-cognition','hept-worker'],load:{paths:[nous,hept]},entries:{
 'nous-cognition':{enabled:true,hooks:{allowConversationAccess:true,allowPromptInjection:true,timeoutMs:120000},config:{runtimeId:'smoke-native',installationId:values['nous-installation-id'],bindings:{},modelResources:{}}},
 'hept-worker':{enabled:true,config:{runtimeId:'smoke-native',subjects:{}}},
}}};
const configPath=path.join(state,'openclaw.json');await writeFile(configPath,`${JSON.stringify(config,null,2)}\n`,{mode:0o600});
const token=randomBytes(32).toString('hex');let logs='',exitCode;
const child=spawn(process.execPath,[path.join(runtime,'openclaw.mjs'),'gateway','run','--port',String(port),'--bind','loopback','--auth','token'],{
 cwd:runtime,detached:process.platform!=='win32',env:{...process.env,OPENCLAW_STATE_DIR:state,OPENCLAW_CONFIG_PATH:configPath,OPENCLAW_GATEWAY_TOKEN:token,OPENCLAW_NO_RESPAWN:'1',NODE_OPTIONS:'--max-old-space-size=1536',GOMAXPROCS:'2',RAYON_NUM_THREADS:'2',TOKIO_WORKER_THREADS:'2'},stdio:['ignore','pipe','pipe'],
});
const append=value=>{logs=(logs+String(value).replaceAll(token,'[redacted]')).slice(-2_000_000);};
child.stdout.on('data',append);child.stderr.on('data',append);
const closed=new Promise(resolve=>{child.once('error',error=>{append(error.message);exitCode=-1;resolve();});child.once('exit',code=>{exitCode=code??-1;resolve();});});
try{
 let ready=false;
 for(let i=0;i<240;i++){
  if(exitCode!==undefined)throw new Error(`Gateway exited during startup (${exitCode})`);
  ready=await fetch(`http://127.0.0.1:${port}/healthz`,{signal:AbortSignal.timeout(1000)}).then(r=>r.status===200).catch(()=>false);
  if(ready)break;await delay(250);
 }
 if(!ready)throw new Error('Gateway startup health deadline elapsed');
 for(const plugin of ['nous','hept']){
  const response=await fetch(`http://127.0.0.1:${port}/plugins/${plugin}/models/readBinding`,{headers:{authorization:`Bearer ${token}`},signal:AbortSignal.timeout(5000)});
  if(response.status!==405)throw new Error(`The ${plugin} authenticated model owner route is not registered (${response.status})`);
 }
 await writeFile(path.join(root,'result.json'),`${JSON.stringify({gateway:'healthy',plugins:['nous-cognition','hept-worker'],modelCalls:0,port},null,2)}\n`,{mode:0o600});
 console.log(JSON.stringify({gateway:'healthy',plugins:['nous-cognition','hept-worker'],modelCalls:0,result:path.join(root,'result.json')}));
}finally{
 if(exitCode===undefined){
  if(process.platform==='win32')child.kill('SIGTERM');else try{process.kill(-child.pid,'SIGTERM');}catch{}
  await Promise.race([closed,delay(10000)]);
  if(exitCode===undefined){if(process.platform==='win32')child.kill('SIGKILL');else try{process.kill(-child.pid,'SIGKILL');}catch{}await Promise.race([closed,delay(5000)]);}
 }
 await writeFile(path.join(root,'gateway.log'),logs,{mode:0o600});
}

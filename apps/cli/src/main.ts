#!/usr/bin/env node
import { Command } from 'commander';
import { readFile,writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { NousClient } from '@nous-wave/client';
import { createNous,makeContext } from '@nous-wave/nous';
import { loadBootstrap,secretFile } from '@nous-wave/nous';
const cli=new Command('nous').description('Nous Wave 认知服务').option('--config <path>','启动配置文件').option('--url <url>','服务地址','http://127.0.0.1:4333');
async function client(){const config=await loadBootstrap(cli.opts().config);const key=await readFile(await secretFile(config.dataRoot,'cli-auth'),'utf8');return new NousClient({baseURL:cli.opts().url,apiKey:key.trim()});}
function show(value:unknown):void {process.stdout.write(JSON.stringify(value,null,2)+'\n');}
cli.command('bootstrap').description('在本机创建初始管理员').requiredOption('--email <email>').option('--name <name>','显示名称','管理员').option('--password-env <name>','密码环境变量','NOUS_ADMIN_PASSWORD').action(async options=>{
  const password=process.env[options.passwordEnv];if(!password)throw new Error('请通过指定环境变量提供管理员密码');const config=await loadBootstrap(cli.opts().config);const app=await createNous(config);
  try{const auth=await app.authentication();const user=await auth.bootstrap({email:options.email,password,name:options.name});const token=await auth.createKey(user.userId,{kind:'human',id:user.userId,grants:['*'],subjectIds:['*']},'local-cli');const path=await secretFile(config.dataRoot,'cli-auth');await writeFile(path,token.key,{mode:0o600});show({created:true,credentialFile:path});}finally{await app.close();}
});
cli.command('doctor').description('检查配置、数据库与当前必要依赖').action(async()=>{const config=await loadBootstrap(cli.opts().config);const app=await createNous(config);try{show(await app.doctor());}finally{await app.close();}});
const subjects=cli.command('subjects').description('主体管理');
subjects.command('create <name>').option('--full','启用完整认知').action(async(name,options)=>show(await(await client()).request('POST','/v1/subjects',{displayName:name,capabilities:options.full?['Memory','Episode','Journal','Schema','Concept','Self','Social','LanguageConvention','Motivation']:['Memory']})));
subjects.command('list').action(async()=>show(await(await client()).request('GET','/v1/subjects')));
subjects.command('read <subject>').action(async subject=>show(await(await client()).request('GET',`/v1/subjects/${subject}`)));
cli.command('query <subject> <text>').description('自然语言或 NousQL 检索').action(async(subject,text)=>show(await(await client()).query(subject,text)));
cli.command('capture <subject> <file>').description('接纳 UTF-8 资料').action(async(subject,file)=>show(await(await client()).capture(subject,{kind:'document',mediaType:'text/plain',title:file,text:await readFile(file,'utf8'),source:{roots:[]}})));
cli.command('remember <subject> <text>').description('管理员直接创建认识').action(async(subject,text)=>show(await(await client()).request('POST',`/v1/subjects/${subject}/knowledge`,{kind:'Memory',body:{kind:'Memory',content:text},sourceRefs:[],formed:{clockId:'real',at:new Date().toISOString()},formation:'operator_edit',stance:'accepted'})));
cli.command('read <subject> <id>').action(async(subject,id)=>show(await(await client()).request('GET',`/v1/subjects/${subject}/knowledge/${id}`)));
cli.command('growth <subject>').description('整理实际主体经历').action(async subject=>show(await(await client()).request('POST',`/v1/subjects/${subject}/growth`,{})));
const models=cli.command('models').description('模型资源');models.command('register <file>').action(async file=>show(await(await client()).request('POST','/v1/models',JSON.parse(await readFile(file,'utf8')))));models.command('list').action(async()=>show(await(await client()).request('GET','/v1/models')));
cli.command('costs <subject>').action(async subject=>show(await(await client()).request('GET',`/v1/costs?subjectId=${subject}`)));
cli.command('operation <subject> <id>').action(async(subject,id)=>show(await(await client()).request('GET',`/v1/operations/${encodeURIComponent(id)}?subjectId=${subject}`)));
await cli.parseAsync();

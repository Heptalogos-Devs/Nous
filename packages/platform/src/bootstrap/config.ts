// Copyright 2026 Aravine Zhu
// SPDX-License-Identifier: Apache-2.0
// Adapted from Nous 412b0620 locations.ts and configuration/file.ts.
import { createHash, randomUUID } from 'node:crypto';
import { access, chmod, link, lstat, mkdir, open, readFile, rm } from 'node:fs/promises';
import { constants } from 'node:fs';
import { dirname } from 'node:path';
import { parseTOML, stringifyTOML } from 'confbox';
import writeFileAtomic from 'write-file-atomic';
import { lock } from 'proper-lockfile';
import { z } from 'zod';
import { ProblemError, type Problem } from '../errors.js';
import { PathOverridesSchema, resolveLocations, resolveLocator, type Paths, type Product } from './locations.js';
import { secureWindowsSecretRoot } from './windows.js';

const reference = z.string().min(1).max(256).regex(/^[A-Za-z0-9][A-Za-z0-9_.:-]*$/);
const port = z.number().int().min(1).max(65535);
const poolSize = z.number().int().min(1).max(1000);
const httpURL=z.url().refine(value=>{const url=new URL(value);return ['http:','https:'].includes(url.protocol)&&!url.username&&!url.password;},'Expected a credential-free HTTP URL');
const external = z.strictObject({ mode:z.literal('external'),urlRef:reference,maxConnections:poolSize });
const managed = z.strictObject({ mode:z.literal('managed'),runtimeRef:reference,port,
  database:z.string().regex(/^[A-Za-z_][A-Za-z0-9_]{0,62}$/),user:z.string().regex(/^[A-Za-z_][A-Za-z0-9_]{0,62}$/),passwordRef:reference,maxConnections:poolSize });
const listen = z.strictObject({ host:z.string().min(1),port,publicBaseURL:httpURL.optional(),trustedOrigins:z.array(httpURL) });
const service = z.strictObject({ mode:z.enum(['foreground','user','system']),autoStartHost:z.boolean() });
const logging = z.strictObject({ level:z.enum(['debug','info','warn','error']) });
export const BootstrapSchema = z.strictObject({ formatVersion:z.literal(1),product:z.enum(['nous','hept']),
  installationId:z.string().min(1).max(256),paths:PathOverridesSchema,
  database:z.discriminatedUnion('mode',[external,managed]),listen,service,logging });
export type Bootstrap = z.infer<typeof BootstrapSchema>;
const ChangesSchema = z.strictObject({ paths:PathOverridesSchema.optional(),
  database:z.discriminatedUnion('mode',[external,managed]).optional(),
  listen:listen.partial().optional(),service:service.partial().optional(),logging:logging.partial().optional() });
export type BootstrapChanges = z.infer<typeof ChangesSchema>;
export type BootstrapSource='default'|'file'|'environment'|'argument';
export interface BootstrapView {
  configPath:string; bootstrap:Bootstrap; paths:Paths; digest:string;
  sources:Record<string,BootstrapSource>;
}
export interface DoctorFinding { code:string;state:'ok'|'warning'|'unavailable';message:string;root?:string }
export interface OfflineDiagnostics {
  product:Product;configPath:string;installationId?:string;
  state:'ok'|'degraded'|'unavailable';findings:DoctorFinding[];
}
export interface BootstrapOptions {
  product:Product;programRoot:string;configPath?:string;instanceHome?:string;
  checks?:(view:BootstrapView)=>Promise<DoctorFinding[]>;
}

function fail(code:string,message:string,status=400,details?:Problem['details']):never {
  throw new ProblemError({code:`bootstrap.${code}`,category:status===409?'conflict':status===403?'forbidden':status===404?'not_found':status===503?'unavailable':'validation',
    status,messageKey:`bootstrap.${code}`,message,...(details?{details}:{})});
}
function parse<T>(schema:z.ZodType<T>,value:unknown):T {
  const result=schema.safeParse(value);
  if(!result.success) fail('invalid_configuration','引导配置格式无效',400,
    {issues:result.error.issues.map(issue=>({path:issue.path.map(String).join('.'),code:issue.code}))});
  return result.data;
}
function digest(text:string) { return createHash('sha256').update(text).digest('hex'); }
function initial(product:Product):Bootstrap {
  return {formatVersion:1,product,installationId:`${product}-${randomUUID()}`,paths:{},
    database:{mode:'external',urlRef:'database-url',maxConnections:10},
    listen:{host:'127.0.0.1',port:product==='nous'?4333:8788,trustedOrigins:[]},
    service:{mode:'foreground',autoStartHost:false},logging:{level:'info'}};
}
function merge(config:Bootstrap,changes:BootstrapChanges):Bootstrap {
  return parse(BootstrapSchema,{...config,...changes,paths:{...config.paths,...changes.paths},
    listen:{...config.listen,...changes.listen},service:{...config.service,...changes.service},logging:{...config.logging,...changes.logging}});
}
function environment(config:Bootstrap):{config:Bootstrap;sources:Record<string,BootstrapSource>} {
  const prefix=config.product==='nous'?'NOUS':'HEPT';
  const sources:Record<string,BootstrapSource>={};
  const changes:BootstrapChanges={};
  const take=(name:string,path:string,apply:(value:string)=>void)=>{
    const value=process.env[`${prefix}_${name}`];if(value!==undefined){apply(value);sources[path]='environment';}
  };
  take('DATA_ROOT','paths.data',value=>{changes.paths={data:value};});
  take('DATABASE_URL_REF','database.urlRef',value=>{
    if(config.database.mode!=='external')fail('environment_conflict','外部数据库引用不能覆盖managed模式');
    changes.database={...config.database,urlRef:value};
  });
  take('HOST','listen.host',value=>{changes.listen={...changes.listen,host:value};});
  take('PORT','listen.port',value=>{changes.listen={...changes.listen,port:Number(value)};});
  take('LOG_LEVEL','logging.level',value=>{changes.logging={level:value as Bootstrap['logging']['level']};});
  const visit=(value:unknown,prefix='')=>{
    if(value&&typeof value==='object'&&!Array.isArray(value))for(const[key,child]of Object.entries(value))visit(child,prefix?`${prefix}.${key}`:key);
    else sources[prefix]??='file';
  };
  const effective=merge(config,changes);visit(effective);return {config:effective,sources};
}
async function readDocument(path:string,product:Product):Promise<{config:Bootstrap;digest:string}> {
  let text:string;
  try {
    const info=await lstat(path);
    if(!info.isFile()||info.isSymbolicLink())fail('invalid_locator','引导文件必须为普通文件');
    if(info.size>262144)fail('configuration_too_large','引导配置超过256KiB');
    text=new TextDecoder('utf-8',{fatal:true}).decode(await readFile(path));
  } catch(error) {
    if(error instanceof ProblemError)throw error;
    const code=(error as NodeJS.ErrnoException).code;
    if(code==='ENOENT')fail('not_initialized','实例尚未初始化',404);
    fail(code==='EACCES'?'access_denied':'read_failed','无法读取引导配置',code==='EACCES'?403:503);
  }
  let value:unknown;try{value=parseTOML(text);}catch{fail('invalid_toml','引导配置不是有效TOML');}
  const config=parse(BootstrapSchema,value);
  if(config.product!==product)fail('product_mismatch','引导文件所属产品不匹配');
  return {config,digest:digest(text)};
}
async function publishNew(path:string,text:string):Promise<boolean> {
  const candidate=`${path}.init-${randomUUID()}`;
  try {
    await writeFileAtomic(candidate,text,{encoding:'utf8',mode:0o600,fsync:true});
    try { await link(candidate,path);return true; }
    catch(error){if((error as NodeJS.ErrnoException).code==='EEXIST')return false;throw error;}
  } finally { await rm(candidate,{force:true}); }
}
async function syncParent(path:string) {
  if(process.platform==='win32')return;
  const directory=await open(dirname(path),'r');try{await directory.sync();}finally{await directory.close();}
}

/** Called by init or the explicit Host startup composition, never by inspect/doctor. */
export async function prepareDirectories(paths:Paths,options:{initializeSecretPermissions?:boolean}={}):Promise<void> {
  for(const[name,path]of Object.entries(paths)) {
    if(name==='program')continue;
    const created=await mkdir(path,{recursive:true,mode:0o700});
    if(name==='secret') {
      const info=await lstat(path);
      if(!info.isDirectory()||info.isSymbolicLink())fail('invalid_secret_root','SecretRoot必须为独立普通目录');
      if(process.platform!=='win32')await chmod(path,0o700);
    }
    if(name==='secret'&&process.platform==='win32'&&(created!==undefined||options.initializeSecretPermissions))await secureWindowsSecretRoot(path);
  }
}

export function createBootstrap(options:BootstrapOptions) {
  const configPath=resolveLocator(options);
  const view=(config:Bootstrap,hash:string):BootstrapView=>{
    const effective=environment(config);
    const paths=resolveLocations({...options,configPath,installationId:config.installationId,paths:effective.config.paths});
    const prefix=options.product==='nous'?'NOUS':'HEPT';
    effective.sources.configPath=options.configPath?'argument':process.env[`${prefix}_CONFIG`]?'environment':options.instanceHome?'argument':'default';
    for(const name of Object.keys(paths))effective.sources[`paths.${name}`]??=name==='program'&&options.programRoot||options.instanceHome?'argument':'default';
    return {configPath,bootstrap:effective.config,digest:hash,sources:effective.sources,paths};
  };
  async function inspect():Promise<BootstrapView> {
    const saved=await readDocument(configPath,options.product);return view(saved.config,saved.digest);
  }
  async function init(input:BootstrapChanges={}):Promise<BootstrapView> {
    let candidate=merge(initial(options.product),parse(ChangesSchema,input));
    const selectedPaths=resolveLocations({...options,configPath,installationId:candidate.installationId,paths:candidate.paths});
    const {program:_program,...stablePaths}=selectedPaths;
    candidate={...candidate,paths:{...stablePaths,...candidate.paths}};
    await mkdir(dirname(configPath),{recursive:true,mode:0o700});
    try {
      const created=await publishNew(configPath,stringifyTOML(candidate));
      if(created)await syncParent(configPath);
      const installed=await inspect();
      await prepareDirectories(installed.paths,{initializeSecretPermissions:created});
      return installed;
    } catch(error) {
      if(error instanceof ProblemError)throw error;
      fail('initialization_failed','实例目录或引导配置初始化失败',503);
    }
  }
  async function configure(input:{expectedDigest:string;changes:BootstrapChanges}):Promise<BootstrapView> {
    const changes=parse(ChangesSchema,input.changes);
    let release:undefined|(()=>Promise<void>);
    try {
      release=await lock(configPath,{realpath:true,retries:{retries:5,minTimeout:20,maxTimeout:200}});
      const saved=await readDocument(configPath,options.product);
      if(saved.digest!==input.expectedDigest)fail('revision_conflict','引导配置已被修改',409);
      const next=merge(saved.config,changes);
      await writeFileAtomic(configPath,stringifyTOML(next),{encoding:'utf8',mode:0o600,fsync:true});
      await syncParent(configPath);
      return await inspect();
    } catch(error) {
      if(error instanceof ProblemError)throw error;
      const code=(error as NodeJS.ErrnoException).code;
      return fail(code==='ELOCKED'?'configuration_busy':'write_failed','无法保存引导配置',code==='ELOCKED'?409:503);
    } finally {await release?.();}
  }
  async function doctor():Promise<OfflineDiagnostics> {
    const findings:DoctorFinding[]=[];
    const major=Number(process.versions.node.split('.')[0]);
    findings.push({code:'bootstrap.node',state:major===24?'ok':'unavailable',message:`Node ${process.versions.node}`});
    let installed:BootstrapView;
    try{installed=await inspect();}
    catch(error){
      findings.push({code:error instanceof ProblemError?error.problem.code:'bootstrap.read_failed',state:'unavailable',message:'无法取得有效引导配置'});
      return {product:options.product,configPath,state:'unavailable',findings};
    }
    for(const[root,path]of Object.entries(installed.paths)) {
      try {
        const info=await lstat(path);
        if(!info.isDirectory()) {findings.push({code:'bootstrap.not_directory',root,state:'unavailable',message:'逻辑根不是目录'});continue;}
        await access(path,root==='program'?constants.R_OK:constants.R_OK|constants.W_OK);
        const insecure=root==='secret'&&process.platform!=='win32'&&(info.mode&0o077)!==0;
        findings.push({code:insecure?'bootstrap.secret_permissions':'bootstrap.directory',root,state:insecure?'unavailable':'ok',message:insecure?'SecretRoot权限必须为0700':'目录可用'});
      } catch {findings.push({code:'bootstrap.directory_unavailable',root,state:root==='program'?'unavailable':'warning',message:'目录不存在或当前账号不可访问'});}
    }
    if(options.checks)try{findings.push(...await options.checks(installed));}catch{findings.push({code:'bootstrap.check_unavailable',state:'warning',message:'附加离线检查不可用'});}
    const state=findings.some(f=>f.state==='unavailable')?'unavailable':findings.some(f=>f.state==='warning')?'degraded':'ok';
    return {product:options.product,configPath,installationId:installed.bootstrap.installationId,state,findings};
  }
  return {configPath,inspect,init,configure,doctor};
}

import { readFile, mkdir, writeFile } from 'node:fs/promises';
import { dirname,join,resolve } from 'node:path';
import { randomBytes } from 'node:crypto';
import { parseTOML } from 'confbox';
import envPaths from 'env-paths';
import { z } from 'zod';
export const BootstrapSchema=z.object({databaseUrl:z.string().min(1),dataRoot:z.string(),installationId:z.string().min(1),host:z.string().default('127.0.0.1'),port:z.number().int().min(1).max(65535).default(4333),logLevel:z.enum(['debug','info','warn','error']).default('info')});
export type Bootstrap=z.infer<typeof BootstrapSchema>;
export async function loadBootstrap(configPath?:string):Promise<Bootstrap> {
  const defaults=envPaths('nous-wave',{suffix:''});const path=configPath??process.env.NOUS_CONFIG??join(defaults.config,'bootstrap.toml');
  let file:Record<string,unknown>={};try{file=parseTOML(await readFile(path,'utf8')) as Record<string,unknown>;}catch(error){if((error as NodeJS.ErrnoException).code!=='ENOENT')throw error;}
  const dataRoot=resolve(process.env.NOUS_DATA_ROOT??String(file.dataRoot??defaults.data));
  await mkdir(dataRoot,{recursive:true,mode:0o700});
  const installationPath=join(dataRoot,'installation-id');let installationId:string;
  try{installationId=(await readFile(installationPath,'utf8')).trim();}catch(error){if((error as NodeJS.ErrnoException).code!=='ENOENT')throw error;installationId=`nous-${randomBytes(8).toString('hex')}`;await writeFile(installationPath,installationId,{mode:0o600,flag:'wx'});}
  return BootstrapSchema.parse({...file,dataRoot,installationId,databaseUrl:process.env.NOUS_DATABASE_URL??file.databaseUrl,host:process.env.NOUS_HOST??file.host,port:process.env.NOUS_PORT?Number(process.env.NOUS_PORT):file.port,logLevel:process.env.NOUS_LOG_LEVEL??file.logLevel});
}
export async function secretFile(dataRoot:string,name:string) {
  if(!/^[a-zA-Z0-9_.-]+$/.test(name))throw new Error('Invalid secret reference');
  const path=join(dataRoot,'secrets',name);await mkdir(dirname(path),{recursive:true,mode:0o700});return path;
}
export async function ensureAuthSecret(dataRoot:string) {const path=await secretFile(dataRoot,'auth');try{return (await readFile(path,'utf8')).trim();}catch(error){if((error as NodeJS.ErrnoException).code!=='ENOENT')throw error;const value=randomBytes(48).toString('base64url');await writeFile(path,value,{flag:'wx',mode:0o600});return value;}}

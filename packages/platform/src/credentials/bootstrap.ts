import { randomBytes } from 'node:crypto';
import { mkdir, readFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { parseTOML, stringifyTOML } from 'confbox';
import lockfile from 'proper-lockfile';
import writeFileAtomic from 'write-file-atomic';
import { v7 as uuid } from 'uuid';
import { z } from 'zod';
import { problem } from '../errors.js';
import { createFileMaterials } from './files.js';

const RefSchema = z.string().regex(/^[a-zA-Z0-9][a-zA-Z0-9_.-]{0,127}$/);
const BindingSchema = z.object({
  purpose:z.string().min(1), generation:z.number().int().positive(),
  backend:z.discriminatedUnion('kind',[
    z.object({kind:z.literal('environment'),variable:z.string().regex(/^[A-Za-z_][A-Za-z0-9_]*$/)}),
    z.object({kind:z.literal('file'),path:z.string().min(1)}),
  ]),
});
const RegistrySchema = z.object({formatVersion:z.literal(1),bindings:z.record(RefSchema,BindingSchema)});
type Registry = z.infer<typeof RegistrySchema>;
type Binding = z.infer<typeof BindingSchema>;

function missing(error:unknown):boolean {
  return error instanceof Error && 'code' in error && error.code==='ENOENT';
}

/** Local Bootstrap/Recovery credentials. No business database or model identity participates. */
export function createBootstrapSecrets(secretRoot:string) {
  const root=resolve(secretRoot);
  const registryPath=join(root,'bootstrap-secrets.toml');
  const files=createFileMaterials(root);

  async function registry():Promise<Registry> {
    try { return RegistrySchema.parse(parseTOML(await readFile(registryPath,'utf8'))); }
    catch(error) {
      if(missing(error)) return {formatVersion:1,bindings:{}};
      throw problem('credentials.bootstrap_invalid','unavailable','引导凭据登记无效');
    }
  }

  async function update<T extends object>(change:(value:Registry)=>Promise<T>):Promise<T&{cleanupPending:boolean}> {
    await mkdir(root,{recursive:true,mode:0o700});
    const release=await lockfile.lock(root,{
      lockfilePath:join(root,'.bootstrap-secrets.lock'),
      retries:{retries:8,minTimeout:10,maxTimeout:200},
    });
    try {
      const value=await registry();
      const before=Object.values(value.bindings).flatMap(binding=>binding.backend.kind==='file'?[binding.backend.path]:[]);
      const result=await change(value);
      const after=Object.values(value.bindings).flatMap(binding=>binding.backend.kind==='file'?[binding.backend.path]:[]);
      try {await writeFileAtomic(registryPath,stringifyTOML(RegistrySchema.parse(value)),{mode:0o600});}
      catch {
        await Promise.allSettled(after.filter(path=>!before.includes(path)).map(path=>files.delete(path)));
        throw problem('credentials.bootstrap_write_failed','unavailable','引导凭据登记保存失败');
      }
      const cleanup=await Promise.allSettled(before.filter(path=>!after.includes(path)).map(path=>files.delete(path)));
      return {...result,cleanupPending:cleanup.some(result=>result.status==='rejected')};
    } finally { await release(); }
  }

  async function binding(ref:string,purpose:string):Promise<Binding> {
    RefSchema.parse(ref);
    const value=(await registry()).bindings[ref];
    if(!value)throw problem('credentials.not_found','not_found','引导凭据尚未登记');
    if(value.purpose!==purpose)
      throw problem('credentials.purpose_denied','forbidden','引导凭据用途不匹配');
    return value;
  }

  return {
    async bindEnv(ref:string,input:{variable:string;purpose:string}) {
      RefSchema.parse(ref);
      return update(async value=>{
        const prior=value.bindings[ref];
        const next=BindingSchema.parse({purpose:input.purpose,generation:(prior?.generation??0)+1,backend:{kind:'environment',variable:input.variable}});
        value.bindings[ref]=next;
        return {ref,generation:next.generation};
      });
    },
    async setFile(ref:string,input:{value:Uint8Array;purpose:string}) {
      RefSchema.parse(ref);
      if(input.value.byteLength===0||input.value.byteLength>65536)
        throw problem('credentials.invalid_material','validation','凭据内容大小无效');
      return update(async value=>{
        const prior=value.bindings[ref];
        const path=join('materials',`${ref}.${uuid()}.secret`);
        await files.put(path,input.value);
        value.bindings[ref]={purpose:input.purpose,generation:(prior?.generation??0)+1,backend:{kind:'file',path}};
        return {ref,generation:value.bindings[ref].generation};
      });
    },
    async ensureFile(ref:string,input:{purpose:string;bytes?:number}) {
      RefSchema.parse(ref);
      return update(async value=>{
        const prior=value.bindings[ref];
        if(prior) {
          if(prior.purpose!==input.purpose)throw problem('credentials.purpose_denied','forbidden','引导凭据用途不匹配');
          return {ref,generation:prior.generation};
        }
        const size=z.number().int().min(32).max(256).parse(input.bytes??48);
        const material=Buffer.from(randomBytes(size).toString('base64url'));
        const path=join('materials',`${ref}.${uuid()}.secret`);
        try {
          await files.put(path,material);
          value.bindings[ref]={purpose:input.purpose,generation:1,backend:{kind:'file',path}};
        } finally { material.fill(0); }
        return {ref,generation:1};
      });
    },
    async withSecret<T>(ref:string,input:{purpose:string},use:(value:Uint8Array)=>Promise<T>):Promise<T> {
      const value=await binding(ref,input.purpose);
      let bytes:Buffer;
      try {
        if(value.backend.kind==='environment') {
          const text=process.env[value.backend.variable];
          if(!text)throw problem('credentials.environment_missing','unavailable','引导凭据环境变量不可用');
          bytes=Buffer.from(text);
        } else return files.withMaterial(value.backend.path,use);
      } catch(error) {
        if(error instanceof Error&&'problem' in error)throw error;
        throw problem('credentials.material_unavailable','unavailable','引导凭据内容不可用');
      }
      if(bytes.length===0||bytes.length>65536) {
        bytes.fill(0);throw problem('credentials.invalid_material','validation','凭据内容大小无效');
      }
      try { return await use(bytes); }
      finally { bytes.fill(0); }
    },
    async status(ref:string) {
      RefSchema.parse(ref);
      const value=(await registry()).bindings[ref];
      if(!value)return {ref,configured:false,available:false};
      let available=false;
      try { available=value.backend.kind==='environment'?Boolean(process.env[value.backend.variable]):await files.exists(value.backend.path); }
      catch { available=false; }
      return {ref,configured:true,available,backend:value.backend.kind,generation:value.generation};
    },
    async revoke(ref:string) {
      RefSchema.parse(ref);
      const result=await update(async value=>{delete value.bindings[ref];return {ref,revoked:true};});
      return result;
    },
  };
}
export type BootstrapSecrets = ReturnType<typeof createBootstrapSecrets>;

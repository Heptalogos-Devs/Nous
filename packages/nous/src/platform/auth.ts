import { betterAuth } from 'better-auth';
import { admin } from 'better-auth/plugins';
import { apiKey } from '@better-auth/api-key';
import { getMigrations } from 'better-auth/db/migration';
import type { Pool } from 'pg';
import type { Actor } from '@nous-wave/contracts/common';
import { ActorSchema } from '@nous-wave/contracts/common';
import { fail,DomainError } from './errors.js';
class AuthenticationRequired extends DomainError { override get status(){return 401;} }
export interface Authentication {
  handler(request:Request):Promise<Response>;
  actor(headers:Headers):Promise<Actor>;
  bootstrap(input:{email:string;password:string;name:string}):Promise<{userId:string}>;
  createKey(userId:string,actor:Actor,name:string):Promise<{id:string;key:string}>;
}
export async function createAuthentication(pool:Pool,secret:string,baseURL:string):Promise<Authentication> {
  const options={database:pool,secret,baseURL,advanced:{cookiePrefix:'nous-wave'},emailAndPassword:{enabled:true},plugins:[admin(),apiKey({enableMetadata:true})]};
  await (await getMigrations(options)).runMigrations();
  const auth=betterAuth(options);
  return {
    handler:request=>auth.handler(request),
    async actor(headers:Headers):Promise<Actor> {
      const key=headers.get('authorization')?.replace(/^Bearer\s+/i,'')??headers.get('x-api-key');
      if(key){const verification=await auth.api.verifyApiKey({body:{key}});if(!verification.valid||!verification.key)throw new AuthenticationRequired('forbidden','API key 无效');const owner=await(await auth.$context).internalAdapter.findUserById(verification.key.referenceId);if(!owner)throw new AuthenticationRequired('forbidden','API key 归属账号不存在');if(!('role' in owner)||owner.role!=='admin')return {kind:'human',id:owner.id,grants:['subjects.read','knowledge.read','materials.read','encounters.read'],subjectIds:[]};const metadata=verification.key.metadata?.actor;return ActorSchema.parse({...metadata,...(metadata?.expiresAt instanceof Date?{expiresAt:metadata.expiresAt.toISOString()}:{})});}
      const session=await auth.api.getSession({headers});if(!session)throw new AuthenticationRequired('forbidden','需要登录');
      return {kind:'human',id:session.user.id,grants:('role' in session.user && session.user.role==='admin')?['*']:['subjects.read','knowledge.read','materials.read','encounters.read'],subjectIds:('role' in session.user && session.user.role==='admin')?['*']:[]};
    },
    async bootstrap(input:{email:string;password:string;name:string}) {
      const ctx=await auth.$context;const existing=await ctx.internalAdapter.findUserByEmail(input.email);
      if(existing)fail('conflict','该管理员账号已经存在');
      const created=await auth.api.signUpEmail({body:input});await ctx.internalAdapter.updateUser(created.user.id,{role:'admin'});return {userId:created.user.id};
    },
    async createKey(userId:string,actor:Actor,name:string) {const result=await auth.api.createApiKey({body:{userId,name,metadata:{actor},permissions:{operations:actor.grants},rateLimitEnabled:true,rateLimitMax:600,rateLimitTimeWindow:60000}});return {id:result.id,key:result.key};},
  };
}

import type { CallContext, Policy, Consumer } from '@nous-wave/contracts/common';
import { fail } from './errors.js';
export const privatePolicy = (subjectId: string): Policy => ({readers:[subjectId],purposes:['*'],audiences:[subjectId]});
export function consumerOf(ctx:CallContext):Consumer {return ctx.consumer??ctx.actor.consumer??{kind:'subject',host:'nous',id:ctx.subjectId};}
export function mayReadScope(ctx:CallContext,scope:Consumer):boolean {
  if(ctx.actor.grants.includes('*')||ctx.actor.grants.includes('data.manage'))return true;
  const consumer=consumerOf(ctx);
  if(scope.kind==='subject')return scope.id===ctx.subjectId;
  return consumer.kind===scope.kind&&consumer.host===scope.host&&consumer.id===scope.id;
}
export function requireConsumer(ctx:CallContext,consumer:Consumer) {
  const trusted=consumerOf(ctx);
  if(consumer.kind==='subject'&&consumer.id!==ctx.subjectId)fail('forbidden','主体消费者身份不符');
  if(ctx.actor.consumer&&(trusted.kind!==consumer.kind||trusted.host!==consumer.host||trusted.id!==consumer.id))fail('forbidden','消费者与宿主委派身份不符');
  if(['worker','author'].includes(ctx.actor.kind)&&consumer.kind!==ctx.actor.kind)fail('forbidden','消费者与执行身份不符');
}
const contains = (scope: string[], value: string) => scope.includes('*') || scope.includes(value);
export function authorize(ctx: CallContext, operation: string) {
  if (ctx.actor.expiresAt && Date.parse(ctx.actor.expiresAt) <= Date.now()) fail('forbidden','授权已过期');
  if (!contains(ctx.actor.grants,operation) && !ctx.actor.grants.includes(`${operation.split('.')[0]}.*`)) fail('forbidden',`缺少 ${operation} 权限`);
  if (ctx.actor.subjectIds && !contains(ctx.actor.subjectIds,ctx.subjectId)) fail('forbidden','主体不在授权范围内');
}
export function mayRead(ctx: CallContext, policy: Policy) {
  const reader = contains(policy.readers,ctx.actor.id) || contains(policy.readers,ctx.subjectId) || ctx.actor.grants.includes('data.manage');
  const purpose = contains(policy.purposes,ctx.purpose);
  const audience = ctx.audience.every(value => contains(policy.audiences,value));
  return reader && purpose && audience;
}
export function intersectPolicy(policies: Policy[]): Policy {
  const intersect = (left:string[],right:string[]) => left.includes('*') ? right : right.includes('*') ? left : left.filter(x=>right.includes(x));
  return policies.reduce((a,b)=>({readers:intersect(a.readers,b.readers),purposes:intersect(a.purposes,b.purposes),audiences:intersect(a.audiences,b.audiences)}),{readers:['*'],purposes:['*'],audiences:['*']});
}

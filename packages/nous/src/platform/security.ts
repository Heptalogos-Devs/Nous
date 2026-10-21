import {readProblem,type CallContext,type Policy,type Consumer} from '@nous-wave/contracts/common';
import { fail } from './errors.js';
export function isAccessDenied(error:unknown):boolean {
  return readProblem(error)?.category==='forbidden';
}
export const privatePolicy = (subjectId: string): Policy => ({readers:[subjectId],purposes:['*'],audiences:[subjectId]});
export function requireSubject(ctx:CallContext):asserts ctx is CallContext&{subjectId:string} {
  if(!ctx.subjectId)fail('invalid_input','该操作需要主体范围');
}
export function mayReadScope(ctx:CallContext,scope:Consumer):boolean {
  if(scope.kind==='subject')return Boolean(ctx.subjectId&&scope.id===ctx.subjectId);
  return ctx.consumer.kind===scope.kind&&ctx.consumer.host===scope.host&&ctx.consumer.id===scope.id;
}
export function requireConsumer(ctx:CallContext,consumer:Consumer) {
  if(ctx.consumer.kind!==consumer.kind||ctx.consumer.host!==consumer.host||ctx.consumer.id!==consumer.id)fail('forbidden','消费者与宿主委派身份不符');
}
const contains=(scope:string[],value:string|undefined)=>value!==undefined&&(scope.includes('*')||scope.includes(value));
export function mayRead(ctx:CallContext,policy:Policy) {
  const reader=contains(policy.readers,ctx.actor.id)||contains(policy.readers,ctx.subjectId);
  return reader&&contains(policy.purposes,ctx.purpose)&&ctx.audience.every(value=>contains(policy.audiences,value));
}
export function intersectPolicy(policies: Policy[]): Policy {
  const intersect = (left:string[],right:string[]) => left.includes('*') ? right : right.includes('*') ? left : left.filter(x=>right.includes(x));
  return policies.reduce((a,b)=>({readers:intersect(a.readers,b.readers),purposes:intersect(a.purposes,b.purposes),audiences:intersect(a.audiences,b.audiences)}),{readers:['*'],purposes:['*'],audiences:['*']});
}

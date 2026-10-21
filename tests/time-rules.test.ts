import assert from 'node:assert/strict';
import test from 'node:test';
import {TimePointSchema} from '@nous-wave/contracts/common';
import {parse,parseInstant,parseRange,compare,contains,overlaps,readReal} from '@nous-wave/platform/time';

test('时间入口保留整微秒并统一时区，拒绝无效日期与精度损失',()=>{
  assert.equal(parseInstant('2026-10-20T09:00:00.123456+09:00'),'2026-10-20T00:00:00.123456Z');
  assert.equal(parseInstant('1969-12-31T23:59:59.999999Z'),'1969-12-31T23:59:59.999999Z');
  assert.equal(parseInstant('2026-10-20T00:00:00.123456000Z'),'2026-10-20T00:00:00.123456Z');
  for(const input of ['2026-02-30T00:00:00Z','2026-10-20T00:00:00','2026-10-20T00:00:00.123456001Z','0000-01-01T00:00:00Z'])assert.throws(()=>parseInstant(input),input);
  assert.equal(TimePointSchema.safeParse({domainId:'real',at:'2026-10-20T00:00:00Z'}).success,false);
  assert.equal(TimePointSchema.safeParse(readReal()).success,true);
});

test('不同时间域没有隐含顺序；范围严格半开并精确到一微秒',()=>{
  const at=(value:string,domainId='campus')=>parse({domainId,at:value});
  const start=at('2026-10-20T00:00:00.123456Z'),end=at('2026-10-20T00:00:00.123457Z');
  const first={domainId:'campus',range:parseRange({start,end})};
  assert.equal(compare(start,end),-1);
  assert.equal(compare(start,{...start,domainId:'real'}),undefined);
  assert.equal(contains(first,start),true);
  assert.equal(contains(first,end),false);
  assert.equal(contains(first,{...start,domainId:'real'}),undefined);
  assert.equal(overlaps(first,{domainId:'campus',range:{start:end}}),false);
  assert.equal(overlaps(first,{domainId:'campus',range:{end}}),true);
  assert.equal(overlaps(first,{domainId:'real',range:{}}),undefined);
  for(const range of [{start,end:start},{start:end,end:start},{start,end:{...end,domainId:'real'}}])assert.throws(()=>parseRange(range));
});

test('deadline投影保留微秒、暂停已到期顺序和异域unknown',async()=>{
  const {projectDeadline,delaySecondsCeil,micros,addMicros}=await import('@nous-wave/platform/time');
  const clock={domainId:'world',rate:'2.5',now:{point:{domainId:'world',at:'2030-01-01T00:00:00.000000Z'},realAt:'2026-01-01T00:00:00.000000Z'}};
  assert.deepEqual(projectDeadline(clock,{domainId:'world',at:'2030-01-01T00:00:00.000011Z'}),{kind:'due',at:'2026-01-01T00:00:00.000005Z'});
  assert.deepEqual(projectDeadline({...clock,rate:'0'},clock.now.point),{kind:'due',at:clock.now.realAt});
  assert.deepEqual(projectDeadline({...clock,rate:'0'},{domainId:'world',at:'2030-01-01T00:00:00.000001Z'}),{kind:'paused'});
  assert.deepEqual(projectDeadline(clock,{...clock.now.point,domainId:'another'}),{kind:'unmapped'});
  assert.equal(delaySecondsCeil('2026-01-01T00:00:00.000001Z',clock.now.realAt),0.001);
  assert.equal(micros('1969-12-31T23:59:59.999999Z'),-1n);assert.equal(addMicros('1969-12-31T23:59:59.999999Z',1n),'1970-01-01T00:00:00.000000Z');
});

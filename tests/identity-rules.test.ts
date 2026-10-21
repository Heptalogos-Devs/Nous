import assert from 'node:assert/strict';
import test from 'node:test';
import {ProblemError} from '@nous-wave/contracts/common';
import {makeContext} from '../packages/nous/src/platform/context.js';
import {isAccessDenied,mayRead} from '../packages/nous/src/platform/security.js';

test('Policy集合不从Actor名称或权限文本取得管理旁路',()=>{
  const ctx=makeContext({actor:{id:'admin',kind:'human',authorityRef:'user:admin'},consumer:{kind:'maintenance',host:'nous',id:'installation'},purpose:'review',audience:['room']});
  assert.equal(mayRead(ctx,{readers:['someone-else'],purposes:['review'],audiences:['room']}),false);
  assert.equal(mayRead(ctx,{readers:['admin'],purposes:['review'],audiences:['room']}),true);
  assert.equal(mayRead(ctx,{readers:['admin'],purposes:['review'],audiences:[]}),false);
});

test('授权拒绝按已验证Problem恢复，不依赖错误实例身份',()=>{
  const failure=new ProblemError({code:'credentials.scope_denied',category:'forbidden',status:403,messageKey:'credentials.scope_denied',message:'当前身份没有操作权限'});
  assert.equal(isAccessDenied(JSON.parse(JSON.stringify(failure))),true);
  assert.equal(isAccessDenied({problem:{category:'forbidden'}}),false);
});

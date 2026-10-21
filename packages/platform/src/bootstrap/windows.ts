import { join, parse } from 'node:path';
import { realpath } from 'node:fs/promises';
import { execa } from 'execa';
import { ProblemError } from '../errors.js';
import type { Paths } from './locations.js';

const windowsBinary=(name:string)=>join(process.env.SystemRoot??'C:\\Windows','System32',name);
async function native(file:string,args:string[]):Promise<string> {
  try {
    const result=await execa(file,args,{reject:false,windowsHide:true,timeout:45_000,maxBuffer:65536,stdin:'ignore'});
    if(result.exitCode===0)return result.stdout;
    throw new ProblemError({code:'bootstrap.windows_acl_failed',category:result.exitCode===5?'forbidden':'unavailable',
      status:result.exitCode===5?403:503,messageKey:'bootstrap.windows_acl_failed',message:'无法取得Windows身份或应用安装目录ACL'});
  } catch(error) {
    if(error instanceof ProblemError)throw error;
    throw new ProblemError({code:'bootstrap.windows_acl_unavailable',category:'unavailable',status:503,
      messageKey:'bootstrap.windows_acl_unavailable',message:'Windows身份/ACL命令不可用'});
  }
}
function readSid(output:string):string {
  const sid=output.match(/S-\d-(?:\d+-)*\d+/)?.[0];
  if(sid)return sid;
  throw new ProblemError({code:'bootstrap.windows_sid_missing',category:'unavailable',status:503,
    messageKey:'bootstrap.windows_sid_missing',message:'Windows没有返回实际账号SID'});
}
async function acl(path:string,...args:string[]) { await native(windowsBinary('icacls.exe'),[path,...args]); }

/** Initial installation only: remove inherited/previous ACLs, then grant the actual owner and SYSTEM. */
export async function secureWindowsSecretRoot(path:string):Promise<void> {
  const sid=readSid(await native(windowsBinary('whoami.exe'),['/user','/fo','csv','/nh']));
  await acl(path,'/reset','/T','/L','/Q');
  await acl(path,'/grant:r',`*${sid}:(OI)(CI)F`,'*S-1-5-18:(OI)(CI)F','/T','/L','/Q');
  await acl(path,'/inheritancelevel:r','/T','/L','/Q');
}

export async function grantWindowsServiceAccess(input:{paths:Paths;configPath:string;account:string;serviceName:string;files:string[]}):Promise<void> {
  const normalized=input.account.replaceAll(' ','').toLowerCase();
  const sid=normalized==='ntauthority\\networkservice'?'S-1-5-20':normalized==='ntauthority\\localservice'?'S-1-5-19':
    readSid(await native(windowsBinary('sc.exe'),['showsid',input.serviceName]));
  const writable=['instance','data','blob','cache','secret','log','run','temp','backup','workspaces','exports'] as const;
  const writablePaths=await Promise.all(writable.map(name=>realpath(input.paths[name])));
  const readablePaths=await Promise.all([input.paths.program,input.paths.runtime].map(path=>realpath(path)));
  for(const path of [...writablePaths,...readablePaths]) {
    if(path===parse(path).root)throw new ProblemError({code:'service.acl_scope_invalid',category:'validation',status:400,
      messageKey:'service.acl_scope_invalid',message:'安装目录不能是磁盘根目录'});
  }
  for(const path of writablePaths)await acl(path,'/grant:r',`*${sid}:(OI)(CI)M`,'/T','/L','/Q');
  for(const path of readablePaths)await acl(path,'/grant:r',`*${sid}:(OI)(CI)RX`,'/T','/L','/Q');
  for(const path of [input.configPath,...input.files])await acl(await realpath(path),'/grant:r',`*${sid}:RX`,'/L','/Q');
}

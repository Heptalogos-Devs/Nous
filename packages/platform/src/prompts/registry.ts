// Copied/adapted from Nous-Wave 412b0620 apps/nous-core/src/model/prompts.ts.
// Copyright 2026 Aravine Zhu; SPDX-License-Identifier: Apache-2.0
import {getStreamAsBuffer} from 'get-stream';
import {open,realpath} from 'node:fs/promises';
import {isAbsolute,relative,resolve,sep} from 'node:path';
import {createHash} from 'node:crypto';
import {problem} from '../errors.js';
export const MAX_PROMPT_BYTES=128*1024;
export function digest(value:string):string{return createHash('sha256').update(value).digest('hex');}
export function checkText(text:string,maxBytes=MAX_PROMPT_BYTES):string {
  const bytes=Buffer.from(text,'utf8');
  if(!text.trim()||bytes.byteLength>maxBytes||new TextDecoder('utf8',{fatal:true}).decode(bytes)!==text)throw problem('prompts.invalid_source','validation','Prompt必须为非空有效UTF-8并符合字节限制');
  return digest(text);
}
export type PromptAsset={id:string;digest:string;text:string};
export class PromptRegistry {
  constructor(private readonly root:string,private readonly overrideRoot?:string){}
  async load(path:string,source:'program'|'config'='program',maxBytes=MAX_PROMPT_BYTES):Promise<PromptAsset>{
    try{
      if(source==='config'&&!this.overrideRoot)throw problem('prompts.edit_root_unavailable','unavailable','Prompt编辑目录未配置');
      const root=await realpath(source==='config'?this.overrideRoot!:this.root);
      const target=await realpath(resolve(root,path));const rel=relative(root,target);
      if(isAbsolute(rel)||rel==='..'||rel.startsWith(`..${sep}`)||!rel)throw problem('prompts.path_forbidden','forbidden','Prompt路径超出资源根');
      const handle=await open(target,'r');
      try{
        const stat=await handle.stat();if(!stat.isFile()||stat.size>maxBytes)throw problem('prompts.invalid_source','validation','Prompt不是普通文件或超出字节限制');
        const bytes=await getStreamAsBuffer(handle.createReadStream({autoClose:false}),{maxBuffer:maxBytes});
        let text:string;try{text=new TextDecoder('utf8',{fatal:true}).decode(bytes);}catch{throw problem('prompts.invalid_source','validation','Prompt编码不是有效UTF-8');}
        return {id:`${source}/${rel.split(sep).join('/')}`,digest:checkText(text,maxBytes),text};
      }finally{await handle.close();}
    }catch(error){
      if(error&&typeof error==='object'&&'problem'in error)throw error;
      const code=error&&typeof error==='object'&&'code'in error?error.code:undefined;
      throw problem(code==='ENOENT'?'prompts.resource_not_found':'prompts.resource_unavailable',code==='ENOENT'?'not_found':'unavailable','Prompt资源无法读取');
    }
  }
}

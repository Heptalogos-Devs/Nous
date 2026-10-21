// Archive transport uses node-tar and Node streams; no bytes enter workflow arguments.
import { createHash } from 'node:crypto';
import { createReadStream,createWriteStream } from 'node:fs';
import { mkdir,rm,stat } from 'node:fs/promises';
import { join,isAbsolute } from 'node:path';
import { Readable,Transform } from 'node:stream';
import { pipeline } from 'node:stream/promises';
import { createInterface } from 'node:readline';
import * as tar from 'tar';
import { problem } from '../errors.js';
export interface ArchiveLimits {maxBundleBytes:number;maxExpandedBytes:number;maxEntries:number}
export const archiveDefaults:Readonly<ArchiveLimits>={maxBundleBytes:1024**3,maxExpandedBytes:2*1024**3,maxEntries:100000};
export function limitedBytes(maxBytes:number){let size=0;return new Transform({transform(chunk:Buffer,_encoding,done){size+=chunk.length;done(size>maxBytes?problem('data.size_limit','resource_pressure','归档数据超过本次字节上限'):null,chunk);}});}
export async function writeJsonLines(path:string,rows:AsyncIterable<unknown>,maxBytes:number){
 const hash=createHash('sha256');let bytes=0;const input=Readable.from((async function*(){for await(const row of rows){const chunk=Buffer.from(JSON.stringify(row)+'\n');bytes+=chunk.length;if(bytes>maxBytes)throw problem('data.size_limit','resource_pressure','领域记录流超过本次字节上限');hash.update(chunk);yield chunk;}})());
 await pipeline(input,createWriteStream(path,{flags:'wx',mode:0o600}));return {digest:hash.digest('hex'),bytes:String(bytes)};
}
export async function* readJsonLines(path:string,maxRecordBytes=64*1024**2):AsyncIterable<unknown>{
 let trailing=0;const lines=new Transform({transform(chunk:Buffer,_encoding,done){let start=0,index:number;while((index=chunk.indexOf(10,start))>=0){if(trailing+index-start>maxRecordBytes){done(problem('data.record_limit','resource_pressure','单条归档记录超过读取上限'));return;}trailing=0;start=index+1;}trailing+=chunk.length-start;if(trailing>maxRecordBytes){done(problem('data.record_limit','resource_pressure','单条归档记录超过读取上限'));return;}done(null,chunk);}});
 const source=createReadStream(path),input=source.pipe(lines),reader=createInterface({input,crlfDelay:Infinity});let failure:unknown;source.on('error',error=>{failure=error;reader.close();});lines.on('error',error=>{failure=error;reader.close();source.destroy();});
 try{for await(const line of reader){if(!line)continue;try{yield JSON.parse(line);}catch{throw problem('data.invalid_jsonl','validation','归档记录不是有效JSON');}}if(failure)throw failure;}finally{reader.close();source.destroy();lines.destroy();}
}
export async function digestFile(path:string){const hash=createHash('sha256'),integrity=createHash('sha512');let bytes=0;for await(const chunk of createReadStream(path)){bytes+=chunk.length;hash.update(chunk);integrity.update(chunk);}return {digest:hash.digest('hex'),integrity:`sha512-${integrity.digest('base64')}`,bytes:String(bytes)};}
export function packArchive(directory:string,paths:string[],limits:ArchiveLimits):Readable{if(paths.length>limits.maxEntries)throw problem('data.entry_limit','resource_pressure','归档成员超过本次上限');const source=tar.create({cwd:directory,portable:true,noMtime:true,gzip:true,strict:true},paths),limit=limitedBytes(limits.maxBundleBytes);source.on('error',()=>limit.destroy(problem('data.pack_failed','unavailable','归档未完整形成')));return source.pipe(limit);}
/** Extraction accepts only this format's files and no links, devices or duplicate names. */
export async function extractArchive(input:Readable,directory:string,limits:ArchiveLimits){
 await mkdir(directory,{recursive:true,mode:0o700});const seen=new Set<string>();let expanded=0;
 try{await pipeline(input,limitedBytes(limits.maxBundleBytes),tar.extract({cwd:directory,strict:true,preservePaths:false,maxDepth:3,maxMetaEntrySize:65536,maxDecompressionRatio:8192,
  filter(path,entry){
   if(isAbsolute(path)||path.includes('\\')||path.split('/').includes('..')||!('type'in entry)||!['File','OldFile'].includes(String(entry.type))||!(/^(?:manifest\.json|components\/[a-z][a-z0-9_-]*\.jsonl|blobs\/[0-9a-f-]{36})$/u).test(path))throw problem('data.archive_path','validation','归档包含非法路径或非普通文件');
   if(seen.has(path))throw problem('data.archive_duplicate','validation','归档重复声明同一路径');seen.add(path);
   expanded+=entry.size;if(seen.size>limits.maxEntries||!Number.isSafeInteger(entry.size)||entry.size<0||expanded>limits.maxExpandedBytes)throw problem('data.archive_limit','resource_pressure','归档展开超过本次成员或字节上限');return true;
  },
 }));if(!seen.has('manifest.json'))throw problem('data.manifest_missing','validation','归档缺少manifest');return [...seen];}
 catch(error){await rm(directory,{recursive:true,force:true});throw error;}
}

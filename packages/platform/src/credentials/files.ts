// Adapted from Nous-Wave's protected secret-file access; generic I/O uses Node and write-file-atomic.
import { lstat, mkdir, readFile, realpath, unlink } from 'node:fs/promises';
import { dirname, relative, resolve, sep, isAbsolute } from 'node:path';
import writeFileAtomic from 'write-file-atomic';
import { problem, ProblemError } from '../errors.js';

export function createFileMaterials(secretRoot:string) {
  const root=resolve(secretRoot);
  function locate(path:string) {
    const file=resolve(root,path),part=relative(root,file);
    if(!part||isAbsolute(part)||part==='..'||part.startsWith(`..${sep}`))
      throw problem('credentials.path_denied','forbidden','凭据路径越过受保护目录');
    return file;
  }
  async function io<T>(run:()=>Promise<T>):Promise<T> {
    try {return await run();}
    catch(error) {if(error instanceof ProblemError)throw error;throw problem('credentials.material_unavailable','unavailable','受保护凭据文件不可用');}
  }
  async function existing(path:string,forRead=true) {
    const file=locate(path),stat=await lstat(file);
    if(!stat.isFile()||stat.isSymbolicLink())throw problem('credentials.path_denied','forbidden','凭据必须是普通文件');
    const actualRoot=await realpath(root),actual=await realpath(file);
    if(!actual.startsWith(actualRoot+sep))throw problem('credentials.path_denied','forbidden','凭据路径越过受保护目录');
    if(forRead&&process.platform!=='win32'&&(stat.mode&0o077)!==0)throw problem('credentials.file_permissions','forbidden','凭据文件权限必须为0600');
    if(forRead&&(stat.size===0||stat.size>65536))throw problem('credentials.invalid_material','validation','凭据内容大小无效');
    return file;
  }
  return {
    async put(path:string,bytes:Uint8Array) {
      if(bytes.byteLength===0||bytes.byteLength>65536)throw problem('credentials.invalid_material','validation','凭据内容大小无效');
      await io(async()=>{
      const file=locate(path);
      await mkdir(dirname(file),{recursive:true,mode:0o700});
      const parent=await realpath(dirname(file)),actualRoot=await realpath(root);
      if(parent!==actualRoot&&!parent.startsWith(actualRoot+sep))throw problem('credentials.path_denied','forbidden','凭据父目录越界');
      const copy=Buffer.from(bytes);
      try {await writeFileAtomic(file,copy,{mode:0o600,fsync:true});}
      finally {copy.fill(0);}
      });
    },
    async exists(path:string) {
      return io(async()=>{try {await existing(path);return true;}
      catch(error) {if(error instanceof Error&&'code' in error&&error.code==='ENOENT')return false;throw error;}});
    },
    async withMaterial<T>(path:string,use:(bytes:Uint8Array)=>Promise<T>):Promise<T> {
      const bytes=await io(async()=>readFile(await existing(path)));
      try {return await use(bytes);}
      finally {bytes.fill(0);}
    },
    async delete(path:string) {
      return io(async()=>{try {await unlink(await existing(path,false));return true;}
      catch(error) {if(error instanceof Error&&'code' in error&&error.code==='ENOENT')return false;throw error;}});
    },
  };
}

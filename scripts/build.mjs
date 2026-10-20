import { cp, mkdir, readFile, writeFile, rm, symlink } from 'node:fs/promises';
import { resolve, join, basename } from 'node:path';
import { spawnSync } from 'node:child_process';
const root=resolve('.');
const product=join(root,'data/product');
const sdkRoot=join(root,'data/packages');
await rm(product,{recursive:true,force:true});
await rm(sdkRoot,{recursive:true,force:true});
await mkdir(product,{recursive:true});
function run(args,cwd=root){const result=spawnSync('corepack',['pnpm',...args],{cwd,stdio:'inherit'});if(result.status!==0)process.exit(result.status??1);}
const libraries=['contracts','client','nous'];
for(const name of libraries){
  await mkdir(join(sdkRoot,name),{recursive:true});
  await symlink(join(root,'packages',name,'node_modules'),join(sdkRoot,name,'node_modules'),'junction');
  run(['exec','tsc','-p',`packages/${name}/tsconfig.json`]);
  const source=JSON.parse(await readFile(`packages/${name}/package.json`,'utf8'));
  const meta={...source};delete meta.publishConfig;delete meta.scripts;
  meta.exports=Object.fromEntries(Object.keys(source.exports).map(key=>{
    const original=source.exports[key].development.replace(/^\.\//,'').replace(/\.ts$/,'.js');
    const output=name==='nous'?`./dist/packages/nous/${original}`:`./dist/${original.replace(/^src\//,'')}`;
    return [key,{types:output.replace(/\.js$/,'.d.ts'),default:output}];
  }));
  for(const [key,value]of Object.entries(meta.dependencies??{}))if(value==='workspace:*')meta.dependencies[key]='0.1.0';
  const sdk=join(sdkRoot,name);await writeFile(join(sdk,'package.json'),JSON.stringify(meta,null,2)+'\n');
  for(const file of ['LICENSE','NOTICE'])await cp(file,join(sdk,file));
  const target=join(product,'packages',name);await cp(sdk,target,{recursive:true,filter:path=>basename(path)!=='node_modules'});
  const runtime={...meta,dependencies:{...meta.dependencies}};
  for(const key of Object.keys(runtime.dependencies))if(key.startsWith('@nous-wave/'))runtime.dependencies[key]='workspace:*';
  await writeFile(join(target,'package.json'),JSON.stringify(runtime,null,2)+'\n');
}
for(const name of ['server','cli']){
  run(['exec','tsc','-p',`apps/${name}/tsconfig.json`]);
  const application=JSON.parse(await readFile(`apps/${name}/package.json`,'utf8'));
  delete application.scripts;
  if(name==='cli')application.bin={nous:'./src/main.js'};
  await writeFile(join(product,'apps',name,'package.json'),JSON.stringify(application,null,2)+'\n');
}
for(const name of libraries)await rm(join(sdkRoot,name,'node_modules'),{force:true});
const manifest=JSON.parse(await readFile('package.json','utf8'));
manifest.scripts={start:'node apps/server/src/main.js',nous:'node apps/cli/src/main.js'};
await writeFile(join(product,'package.json'),JSON.stringify(manifest,null,2)+'\n');
await cp('pnpm-lock.yaml',join(product,'pnpm-lock.yaml'));
await cp('pnpm-workspace.yaml',join(product,'pnpm-workspace.yaml'));
for(const file of ['LICENSE','NOTICE','README.md'])await cp(file,join(product,file));
run(['install','--offline','--prod','--no-frozen-lockfile','--reporter=append-only'],product);

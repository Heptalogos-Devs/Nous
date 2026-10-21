import { createHash, randomUUID } from 'node:crypto';
import { access, mkdir, readFile, rm } from 'node:fs/promises';
import { homedir } from 'node:os';
import { dirname, isAbsolute, join } from 'node:path';
import { execa } from 'execa';
import writeFileAtomic from 'write-file-atomic';
import { ProblemError } from '../errors.js';
import { productNames } from './locations.js';
import type { BootstrapView } from './config.js';
import { grantWindowsServiceAccess } from './windows.js';

export interface ServiceOptions {
  view:BootstrapView;nodePath?:string;entrypoint?:string;args?:readonly string[];
  scope:'user'|'system';account?:string;shawlPath?:string;
}
export interface ServicePlan {
  manager:'systemd'|'scm'|'launchd';name:string;scope:'user'|'system';
  command:readonly string[];workingDirectory:string;configPath:string;
  account:string;file?:string;definition?:string|Record<string,unknown>;
}
export interface ServiceResult {
  name:string;manager:ServicePlan['manager'];installed:boolean;
  state:'active'|'inactive'|'starting'|'stopping'|'paused'|'failed'|'unknown'|'not_installed';
}
function fail(code:string,message:string,status=503):never {
  throw new ProblemError({code:`service.${code}`,category:status===400?'validation':status===403?'forbidden':status===409?'conflict':'unavailable',
    status,messageKey:`service.${code}`,message});
}
function singleLine(value:string):string {
  if(!value||/[\0\r\n]/.test(value))fail('invalid_value','服务参数不能含空值或控制换行',400);
  return value;
}
function unitQuote(value:string,exec=false) {
  let escaped=singleLine(value).replaceAll('\\','\\\\').replaceAll('"','\\"').replaceAll('%','%%');
  if(exec)escaped=escaped.replaceAll('$','$$');
  return `"${escaped}"`;
}

/** Uses native managers only; the product Host owns its drain/stop semantics. */
export function createServiceManager(options:ServiceOptions) {
  const {view}=options;
  const nodePath=options.nodePath??process.execPath;
  const command=options.entrypoint?[nodePath,options.entrypoint,...(options.args??[])].map(singleLine):undefined;
  const name=`${productNames[view.bootstrap.product]}-${createHash('sha256').update(view.bootstrap.installationId).digest('hex').slice(0,20)}`;
  const prefix=view.bootstrap.product==='nous'?'NOUS':'HEPT';
  const configEnv=`${prefix}_CONFIG`;
  const cwd=view.paths.instance;
  let plan:Omit<ServicePlan,'command'> & {command?:readonly string[]};
  if(process.platform==='linux') {
    const unit=`${name}.service`;
    const file=join(options.scope==='system'?'/etc/systemd/system':join(process.env.XDG_CONFIG_HOME??join(homedir(),'.config'),'systemd','user'),unit);
    const definition=command?['[Unit]',`Description=${productNames[view.bootstrap.product]}`,'After=network.target','',
      '[Service]','Type=exec',`WorkingDirectory=${singleLine(cwd).replaceAll('%','%%')}`,
      `ExecStart=${command.map(arg=>unitQuote(arg,true)).join(' ')}`,
      `Environment=${unitQuote(`${configEnv}=${view.configPath}`)}`,
      ...(options.scope==='system'&&options.account?[`User=${singleLine(options.account)}`]:[]),
      'UMask=0077','Restart=on-failure','RestartSec=5s','TimeoutStopSec=30s','KillSignal=SIGTERM','',
      '[Install]',`WantedBy=${options.scope==='system'?'multi-user.target':'default.target'}`,''].join('\n'):undefined;
    plan={manager:'systemd',name,scope:options.scope,command,workingDirectory:cwd,configPath:view.configPath,
      account:options.scope==='user'?'current_user':options.account??'',file,definition};
  } else if(process.platform==='darwin') {
    const file=join(options.scope==='system'?'/Library/LaunchDaemons':join(homedir(),'Library','LaunchAgents'),`${name}.plist`);
    const definition={Label:name,ProgramArguments:command,WorkingDirectory:cwd,
      EnvironmentVariables:{[configEnv]:view.configPath},RunAtLoad:view.bootstrap.service.autoStartHost,
      KeepAlive:{SuccessfulExit:false},ExitTimeOut:30,Umask:0o077,
      StandardOutPath:join(view.paths.log,'service.stdout.log'),StandardErrorPath:join(view.paths.log,'service.stderr.log'),
      ...(options.scope==='system'&&options.account?{UserName:singleLine(options.account)}:{})};
    plan={manager:'launchd',name,scope:options.scope,command,workingDirectory:cwd,configPath:view.configPath,
      account:options.scope==='user'?'current_user':options.account??'',file,definition};
  } else if(process.platform==='win32') {
    if(options.scope!=='system')fail('scope_unavailable','Windows服务使用SCM system范围',400);
    const account=options.account??'NT AUTHORITY\\NetworkService';
    if(!['NT AUTHORITY\\NetworkService','NT AUTHORITY\\LocalService',`NT SERVICE\\${name}`].includes(account))
      fail('account_unavailable','此入口支持Windows内置或本服务虚拟账号；其他账号由SCM明确配置',400);
    plan={manager:'scm',name,scope:'system',command,workingDirectory:cwd,configPath:view.configPath,account};
  } else fail('platform_unavailable','当前系统没有已实现的原生服务适配');

  function executionPlan():ServicePlan {
    if(!command||!isAbsolute(nodePath)||!options.entrypoint||!isAbsolute(options.entrypoint))fail('absolute_command_required','Node和服务入口必须使用绝对路径',400);
    if(plan.manager!=='scm'&&options.scope==='system'&&!options.account)fail('account_required','系统服务必须明确指定运行账号',400);
    if(plan.manager==='scm'&&(!options.shawlPath||!isAbsolute(options.shawlPath)))fail('shawl_required','需要已安装Shawl的绝对路径',400);
    return structuredClone({...plan,command});
  }

  async function run(file:string,args:readonly string[],input?:string) {
    try {
      return await execa(file,[...args],{reject:false,timeout:45_000,maxBuffer:1024*1024,
        windowsHide:true,...(input!==undefined?{input}:{stdin:'ignore'})});
    } catch {fail('manager_unavailable','无法调用系统服务管理器');}
  }
  async function required(file:string,args:readonly string[],input?:string) {
    const result=await run(file,args,input);
    if(result.exitCode!==0)fail('manager_rejected','系统服务管理器未完成操作');
    return result;
  }
  const systemctl=(args:readonly string[])=>run('systemctl',[...(options.scope==='user'?['--user']:[]),...args]);
  const systemctlRequired=(args:readonly string[])=>required('systemctl',[...(options.scope==='user'?['--user']:[]),...args]);
  const launchDomain=options.scope==='system'?'system':`gui/${process.getuid?.()}`;
  const launchTarget=`${launchDomain}/${name}`;
  const sc=()=>join(process.env.SystemRoot??'C:\\Windows','System32','sc.exe');
  async function status():Promise<ServiceResult> {
    if(plan.manager==='systemd') {
      const result=await systemctl(['show',`${name}.service`,'--property=LoadState,ActiveState','--no-pager']);
      const fields=Object.fromEntries(result.stdout.split('\n').filter(line=>line.includes('=')).map(line=>line.split('=',2)));
      if(fields.LoadState==='not-found')return {name,manager:plan.manager,installed:false,state:'not_installed'};
      if(result.exitCode!==0)fail('manager_unavailable','无法读取systemd服务状态');
      const state=fields.ActiveState==='active'?'active':fields.ActiveState==='activating'?'starting':fields.ActiveState==='deactivating'?'stopping':fields.ActiveState==='inactive'?'inactive':fields.ActiveState==='failed'?'failed':'unknown';
      return {name,manager:plan.manager,installed:true,state};
    }
    if(plan.manager==='launchd') {
      const result=await run('launchctl',['print',launchTarget]);
      if(result.exitCode!==0) {
        try{await access(plan.file!);return {name,manager:plan.manager,installed:true,state:'inactive'};}
        catch{return {name,manager:plan.manager,installed:false,state:'not_installed'};}
      }
      return {name,manager:plan.manager,installed:true,state:/state\s*=\s*running/.test(result.stdout)?'active':'inactive'};
    }
    const script=`try{$s=Get-Service -Name '${name}' -ErrorAction Stop;@{state=[int]$s.Status}|ConvertTo-Json -Compress}catch{if($_.FullyQualifiedErrorId -like 'NoServiceFoundForGivenName*'){'{}'}else{exit 1}}`;
    const result=await required(join(process.env.SystemRoot??'C:\\Windows','System32','WindowsPowerShell','v1.0','powershell.exe'),['-NoProfile','-NonInteractive','-Command',script]);
    let value:{state?:number};try{value=JSON.parse(result.stdout);}catch{fail('invalid_status','SCM状态输出无效');}
    if(value.state===undefined)return {name,manager:plan.manager,installed:false,state:'not_installed'};
    const states:Record<number,ServiceResult['state']>={1:'inactive',2:'starting',3:'stopping',4:'active',5:'starting',6:'stopping',7:'paused'};
    return {name,manager:plan.manager,installed:true,state:states[value.state]??'unknown'};
  }
  async function install():Promise<ServiceResult> {
    const selected=executionPlan();
    await access(selected.command[0]!);await access(selected.command[1]!);
    if(plan.manager==='systemd') {
      await mkdir(dirname(plan.file!),{recursive:true});
      await writeFileAtomic(plan.file!,plan.definition as string,{mode:0o644,fsync:true});
      await systemctlRequired(['daemon-reload']);
      await systemctlRequired([view.bootstrap.service.autoStartHost?'enable':'disable',`${name}.service`]);
    } else if(plan.manager==='launchd') {
      await mkdir(view.paths.temp,{recursive:true,mode:0o700});
      const candidate=join(view.paths.temp,`launchd-${randomUUID()}.plist`);
      try {
        await writeFileAtomic(candidate,JSON.stringify(plan.definition),{mode:0o600});
        await required('/usr/bin/plutil',['-convert','xml1',candidate]);
        await mkdir(dirname(plan.file!),{recursive:true});
        await writeFileAtomic(plan.file!,await readFile(candidate),{mode:0o644,fsync:true});
      } finally {await rm(candidate,{force:true});}
    } else {
      if((await status()).installed)fail('already_installed','服务已经安装；配置更新请使用原生SCM',409);
      await required(options.shawlPath!,['add','--name',name,'--cwd',cwd,'--env',`${configEnv}=${view.configPath}`,
        '--stop-timeout','30000','--no-log-cmd','--',...selected.command]);
      try {
        await grantWindowsServiceAccess({paths:view.paths,configPath:view.configPath,account:plan.account,serviceName:name,
          files:[nodePath,options.entrypoint!,options.shawlPath!]});
        await required(sc(),['config',name,'obj=',plan.account,'password=','','start=',view.bootstrap.service.autoStartHost?'auto':'demand']);
      } catch(error) { await run(sc(),['delete',name]);throw error; }
    }
    return status();
  }
  async function start():Promise<ServiceResult> {
    if(plan.manager==='systemd')await systemctlRequired(['start',`${name}.service`]);
    else if(plan.manager==='scm') {if((await status()).state!=='active')await required(sc(),['start',name]);}
    else {
      const loaded=await run('launchctl',['print',launchTarget]);
      if(loaded.exitCode!==0)await required('launchctl',['bootstrap',launchDomain,plan.file!]);
      await required('launchctl',['kickstart',launchTarget]);
    }
    return status();
  }
  async function stop():Promise<ServiceResult> {
    if(!(await status()).installed)return {name,manager:plan.manager,installed:false,state:'not_installed'};
    if(plan.manager==='systemd')await systemctlRequired(['stop',`${name}.service`]);
    else if(plan.manager==='scm') {if((await status()).state!=='inactive')await required(sc(),['stop',name]);}
    else {const loaded=await run('launchctl',['print',launchTarget]);if(loaded.exitCode===0)await required('launchctl',['bootout',launchTarget]);}
    return status();
  }
  async function uninstall():Promise<ServiceResult> {
    const current=await status();if(!current.installed)return current;
    await stop();
    if(plan.manager==='systemd') {await systemctlRequired(['disable',`${name}.service`]);await rm(plan.file!,{force:true});await systemctlRequired(['daemon-reload']);}
    else if(plan.manager==='launchd')await rm(plan.file!,{force:true});
    else if((await status()).installed)await required(sc(),['delete',name]);
    return status();
  }
  const checked=<T>(fn:()=>Promise<T>)=>async()=>{
    try{return await fn();}catch(error){
      if(error instanceof ProblemError)throw error;
      fail((error as NodeJS.ErrnoException).code==='EACCES'?'access_denied':'filesystem_failed','服务文件或运行依赖不可访问',(error as NodeJS.ErrnoException).code==='EACCES'?403:503);
    }
  };
  return {plan:executionPlan,install:checked(install),start:checked(start),stop:checked(stop),status:checked(status),uninstall:checked(uninstall)};
}

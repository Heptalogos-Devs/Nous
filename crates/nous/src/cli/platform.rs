use super::print_result;
use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use nous_app::App;
use nous_domain::CallContext;
use nous_foundation::{
    instance::{InstanceGuard, InstancePaths},
    network::NetworkClients,
    platform::{Platform, installation::{ExecutableSource, InstallRequest, Installations, StagedUpdate},
        service::{ServiceAction, ServiceScope, ServiceSpec, Services}},
};
use serde_json::json;
use std::path::PathBuf;
use tokio_util::sync::CancellationToken;

#[derive(Args)]
pub struct SourceArgs {
    /// Local executable; defaults to this running Nous executable.
    #[arg(long, conflicts_with="download_url")]
    source: Option<PathBuf>,
    #[arg(long, requires="sha256")]
    download_url: Option<String>,
    #[arg(long, requires="download_url")]
    sha256: Option<String>,
}
impl SourceArgs {
    fn resolve(self)->Result<ExecutableSource> {
        Ok(match (self.source,self.download_url,self.sha256) {
            (_,Some(url),Some(sha256))=>ExecutableSource::Download{url,sha256},
            (source,None,None)=>ExecutableSource::Local{path:std::path::absolute(source.unwrap_or(std::env::current_exe()?))?},
            _=>anyhow::bail!("download requires both --download-url and --sha256"),
        })
    }
}
#[derive(Subcommand)]
pub enum InstallationCommand {
    /// Install the program separately from the selected instance data.
    Install {
        #[arg(long)] program_root: PathBuf,
        #[command(flatten)] source:SourceArgs,
    },
    Inspect { #[arg(long)] program_root:PathBuf },
    /// Move the software inventory; instance data keeps its location.
    Relocate { #[arg(long)] from:PathBuf, #[arg(long)] to:PathBuf },
    /// Remove the managed software inventory; instance data stays available.
    Uninstall { #[arg(long)] program_root:PathBuf },
}
#[derive(Subcommand)]
pub enum UpdateCommand {
    Stage {
        #[arg(long)] program_root:PathBuf,
        #[command(flatten)] source:SourceArgs,
    },
    /// Activate a StagedUpdate JSON record produced by update stage --json.
    Activate { #[arg(long)] file:PathBuf },
    Rollback {
        #[arg(long)] program_root:PathBuf,
        /// Retained executable digest shown by installation inspect.
        #[arg(long)] release:String,
    },
}
#[derive(Args)]
pub struct ServiceArgs {
    /// Absolute or working-directory-relative installed Nous executable.
    #[arg(long)] program:PathBuf,
    #[arg(long,default_value="user",value_parser=parse_scope)] scope:ServiceScope,
    /// Configure login/boot activation when installing the service.
    #[arg(long)] autostart:bool,
}
impl ServiceArgs {
    fn spec(self)->Result<ServiceSpec> {
        Ok(ServiceSpec{program:std::path::absolute(self.program)?,scope:self.scope,autostart:self.autostart})
    }
}
#[derive(Subcommand)]
pub enum ServiceCommand {
    Plan(ServiceArgs),
    Install(ServiceArgs),
    Status(ServiceArgs),
    Start(ServiceArgs),
    Stop(ServiceArgs),
    Restart(ServiceArgs),
    Uninstall(ServiceArgs),
}

async fn installer(paths:&InstancePaths)->Result<Installations> {
    let policy=App::desired_instance_configuration(paths).await?;
    Ok(Installations::new(paths.clone(),policy.installation,NetworkClients::new(&policy.network)?)?)
}
pub async fn installation(paths:InstancePaths,command:InstallationCommand)->Result<()> {
    let owner=installer(&paths).await?;
    let ctx=CallContext::admin();
    let cancel=CancellationToken::new();
    let value=match command {
        InstallationCommand::Install{program_root,source}=>json!(owner.install(&ctx,InstallRequest{program_root:std::path::absolute(program_root)?,source:source.resolve()?},&cancel).await?),
        InstallationCommand::Inspect{program_root}=>json!(owner.inspect(&std::path::absolute(program_root)?,&cancel).await?),
        InstallationCommand::Relocate{from,to}=>json!(owner.relocate(&ctx,&std::path::absolute(from)?,&std::path::absolute(to)?,&cancel).await?),
        InstallationCommand::Uninstall{program_root}=>{
            owner.uninstall(&ctx,&std::path::absolute(&program_root)?,&cancel).await?;
            json!({"uninstalled":program_root,"instance_root":paths.root})
        }
    };
    print_result(value)
}
pub async fn update(paths:InstancePaths,command:UpdateCommand)->Result<()> {
    let owner=installer(&paths).await?;
    let ctx=CallContext::admin();
    let cancel=CancellationToken::new();
    let value=match command {
        UpdateCommand::Stage{program_root,source}=>json!(owner.stage(&ctx,&std::path::absolute(program_root)?,source.resolve()?,&cancel).await?),
        UpdateCommand::Activate{file}=>{
            let staged:StagedUpdate=serde_json::from_slice(&std::fs::read(&file)?)
                .with_context(||format!("invalid staged update {}",file.display()))?;
            json!(owner.activate(&ctx,&staged,&cancel).await?)
        }
        UpdateCommand::Rollback{program_root,release}=>json!(owner.rollback(&ctx,&std::path::absolute(program_root)?,&release,&cancel).await?),
    };
    print_result(value)
}
pub async fn service(paths:InstancePaths,command:ServiceCommand)->Result<()> {
    let config=App::desired_instance_configuration(&paths).await?;
    let platform=Platform::new(paths.clone(),config.platform)?;
    let owner=Services::new(paths.clone(),config.service,platform.clone())?;
    let ctx=CallContext::admin();
    let cancel=CancellationToken::new();
    let result=async {
        let (args,action)=match command {
            ServiceCommand::Plan(args)=>return Ok(json!(owner.plan(&ctx,&args.spec()?)?)),
            ServiceCommand::Install(args)=>return Ok(json!(owner.install(&ctx,&args.spec()?,&cancel).await?)),
            ServiceCommand::Status(args)=>return Ok(json!(owner.status(&ctx,&args.spec()?,&cancel).await?)),
            ServiceCommand::Start(args)=>(args,ServiceAction::Start),
            ServiceCommand::Stop(args)=>(args,ServiceAction::Stop),
            ServiceCommand::Restart(args)=>(args,ServiceAction::Restart),
            ServiceCommand::Uninstall(args)=>(args,ServiceAction::Uninstall),
        };
        let spec=args.spec()?;
        if cfg!(windows) && spec.scope==ServiceScope::User
            && matches!(action,ServiceAction::Stop|ServiceAction::Restart|ServiceAction::Uninstall)
            && InstanceGuard::is_running(&paths)?
        {
            crate::bootstrap::connect(&paths,None,None).await?.stop().await?;
            owner.wait_stopped(&ctx,&spec,&cancel).await?;
        }
        Ok::<_,anyhow::Error>(json!(owner.action(&ctx,&spec,action,&cancel).await?))
    }.await;
    platform.shutdown().await;
    print_result(result?)
}
fn parse_scope(value:&str)->Result<ServiceScope,String> {
    serde_json::from_value(json!(value)).map_err(|error|error.to_string())
}

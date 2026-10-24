mod bootstrap;
mod cli;
mod http;
mod mcp;
use clap::Parser;
#[tokio::main]
async fn main() {
    let logging = match nous_foundation::observability::Observability::install() {
        Ok(guard) => guard,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    };
    let result = cli::execute(cli::Cli::parse()).await;
    if let Err(error) = &result {
        tracing::error!(event="process.failed", error=%error, "command failed");
        eprintln!("{error:#}");
    }
    drop(logging);
    if result.is_err() {
        std::process::exit(1);
    }
}

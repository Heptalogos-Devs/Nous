mod bootstrap;
mod cli;
mod http;
mod mcp;
use clap::Parser;
#[tokio::main]
async fn main() {
    // Adapted from rmcp's counter_stdio example: stdout belongs exclusively to MCP.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "nous=info,nous_app=info,nous_storage=info".into()),
        )
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .json()
        .init();
    if let Err(error) = cli::execute(cli::Cli::parse()).await {
        eprintln!("{error:#}");
        std::process::exit(1);
    }
}

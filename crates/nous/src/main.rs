mod bootstrap;
mod cli;
mod http;
mod mcp;
mod presentation;
use clap::Parser;
#[tokio::main]
async fn main() {
    let logging = match nous_foundation::observability::Observability::install() {
        Ok(guard) => guard,
        Err(error) => {
            eprintln!(
                "{}",
                presentation::error_text(&error, &presentation::current())
            );
            std::process::exit(1);
        }
    };
    let result = cli::execute(cli::Cli::parse()).await;
    if let Err(error) = &result {
        tracing::error!(event="process.failed", error=%error, "command failed");
        let domain = error
            .chain()
            .find_map(|cause| cause.downcast_ref::<nous_domain::Error>())
            .cloned()
            .unwrap_or_else(|| {
                nous_domain::Error::new(
                    nous_domain::ErrorCode::DependencyUnavailable,
                    error.to_string(),
                )
                .with_cause(error.as_ref())
            });
        let locale = presentation::current();
        if cli::json_output() {
            eprintln!("{}", presentation::error_document(&domain, &locale));
        } else {
            eprintln!("{}", presentation::error_text(&domain, &locale));
        }
    }
    drop(logging);
    if result.is_err() {
        std::process::exit(1);
    }
}

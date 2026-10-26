use nous_foundation::localization::{LocalizationPolicy, Localizer, Presentation};
use std::sync::{LazyLock, OnceLock};

static CLI: OnceLock<Presentation> = OnceLock::new();
static CLI_LANGUAGE: OnceLock<Option<String>> = OnceLock::new();
static DEFAULT: LazyLock<Presentation> = LazyLock::new(|| {
    Localizer::new(LocalizationPolicy::default())
        .and_then(|value| value.select(&[]))
        .expect("packaged localization resources")
});
tokio::task_local! { static REQUEST: Presentation; }

pub fn set_cli(presentation: Presentation, explicit: Option<String>) -> nous_domain::Result<()> {
    CLI_LANGUAGE
        .set(explicit)
        .map_err(|_| nous_domain::Error::internal("CLI language was already initialized"))?;
    CLI.set(presentation)
        .map_err(|_| nous_domain::Error::internal("CLI presentation was already initialized"))
}
pub fn current() -> Presentation {
    REQUEST
        .try_with(Clone::clone)
        .unwrap_or_else(|_| CLI.get().unwrap_or(&DEFAULT).clone())
}
pub async fn in_language<F: std::future::Future>(presentation: Presentation, work: F) -> F::Output {
    REQUEST.scope(presentation, work).await
}
pub fn cli_language() -> Option<&'static str> {
    CLI_LANGUAGE.get().and_then(Option::as_deref)
}

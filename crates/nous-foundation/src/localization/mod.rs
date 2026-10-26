mod policy;
pub use fluent_bundle::FluentArgs;
use fluent_bundle::{FluentResource, concurrent::FluentBundle};
use fluent_langneg::{NegotiationStrategy, negotiate_languages};
use nous_domain::{Error, ErrorCode, Result};
pub use policy::{LOCALIZATION, LocalizationPolicy, register_configuration};
use std::sync::{Arc, LazyLock};
use unic_langid::LanguageIdentifier;

const AVAILABLE: &[&str] = &["en-US", "zh-CN"];
const RESOURCES: &[(&str, &str)] = &[
    ("en-US", include_str!("locales/en-US.ftl")),
    ("zh-CN", include_str!("locales/zh-CN.ftl")),
];
struct Catalog {
    languages: Vec<LanguageIdentifier>,
    bundles: Vec<FluentBundle<FluentResource>>,
}
static CATALOG: LazyLock<Result<Arc<Catalog>>> = LazyLock::new(build_catalog);

#[derive(Clone)]
pub struct Localizer {
    catalog: Arc<Catalog>,
    default: LanguageIdentifier,
}
#[derive(Clone)]
pub struct Presentation {
    catalog: Arc<Catalog>,
    order: Vec<usize>,
    locale: String,
}
impl Localizer {
    pub fn new(policy: LocalizationPolicy) -> Result<Self> {
        policy.validate()?;
        let default = policy.default_locale.parse().map_err(|error| {
            Error::invalid("invalid default language identifier").with_cause(&error)
        })?;
        let catalog = CATALOG.as_ref().map_err(Clone::clone)?.clone();
        Ok(Self { catalog, default })
    }
    pub fn available_locales(&self) -> &'static [&'static str] {
        AVAILABLE
    }
    pub fn select_accept_language(&self, header: &str) -> Result<Presentation> {
        let languages = accept_language::parse_with_quality(header)
            .into_iter()
            .filter(|(language, quality)| {
                language != "*" && quality.is_finite() && *quality > 0.0 && *quality <= 1.0
            })
            .filter_map(|(language, _)| {
                language
                    .parse::<LanguageIdentifier>()
                    .ok()
                    .map(|value| value.to_string())
            })
            .collect::<Vec<_>>();
        self.select(&languages.iter().map(String::as_str).collect::<Vec<_>>())
    }
    pub fn select(&self, requested: &[&str]) -> Result<Presentation> {
        let requested = requested
            .iter()
            .map(|value| {
                value.parse::<LanguageIdentifier>().map_err(|error| {
                    Error::invalid("invalid requested language identifier")
                        .with_cause(&error)
                        .with_context("locale", *value)
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let languages = negotiate_languages(
            &requested,
            &self.catalog.languages,
            Some(&self.default),
            NegotiationStrategy::Filtering,
        );
        let mut order = languages
            .iter()
            .filter_map(|language| {
                self.catalog
                    .languages
                    .iter()
                    .position(|available| available == *language)
            })
            .collect::<Vec<_>>();
        if !order.contains(&0) {
            order.push(0);
        }
        let locale = self.catalog.languages[order[0]].to_string();
        Ok(Presentation {
            catalog: self.catalog.clone(),
            order,
            locale,
        })
    }
}
impl Presentation {
    pub fn locale(&self) -> &str {
        &self.locale
    }
    pub fn message(&self, key: &str, args: Option<&FluentArgs<'_>>) -> Result<String> {
        for index in &self.order {
            let bundle = &self.catalog.bundles[*index];
            let Some(message) = bundle.get_message(key) else {
                continue;
            };
            let Some(pattern) = message.value() else {
                continue;
            };
            let mut errors = Vec::new();
            let text = bundle.format_pattern(pattern, args, &mut errors);
            if !errors.is_empty() {
                return Err(
                    Error::internal("localized message arguments could not be formatted")
                        .with_context("message_key", key)
                        .with_context("locale", self.locale.clone())
                        .with_context("format_errors", format!("{errors:?}")),
                );
            }
            return Ok(text.into_owned());
        }
        Err(Error::internal("localized message is not registered")
            .with_context("message_key", key)
            .with_context("locale", self.locale.clone()))
    }
    pub fn error(&self, error: &Error) -> Result<String> {
        let key = match error.code {
            ErrorCode::InvalidArgument => "error-invalid-argument",
            ErrorCode::Unauthorized => "error-unauthorized",
            ErrorCode::Forbidden => "error-forbidden",
            ErrorCode::NotFound => "error-not-found",
            ErrorCode::Conflict => "error-conflict",
            ErrorCode::Cancelled => "error-cancelled",
            ErrorCode::CapabilityUnavailable => "error-capability-unavailable",
            ErrorCode::ResourceExhausted => "error-resource-exhausted",
            ErrorCode::DependencyUnavailable => "error-dependency-unavailable",
            ErrorCode::Internal => "error-internal",
        };
        self.message(key, None)
    }
}
fn build_catalog() -> Result<Arc<Catalog>> {
    let mut languages = Vec::new();
    let mut bundles = Vec::new();
    for (locale, source) in RESOURCES {
        let language: LanguageIdentifier = locale.parse().map_err(|error| {
            Error::internal("packaged language identifier is invalid").with_cause(&error)
        })?;
        let resource = FluentResource::try_new((*source).into()).map_err(|(_, errors)| {
            Error::internal("packaged language resources are invalid")
                .with_context("parse_errors", format!("{errors:?}"))
        })?;
        let mut bundle = FluentBundle::new_concurrent(vec![language.clone()]);
        bundle.add_resource(resource).map_err(|errors| {
            Error::internal("packaged language resources contain duplicate messages")
                .with_context("registration_errors", format!("{errors:?}"))
        })?;
        languages.push(language);
        bundles.push(bundle);
    }
    Ok(Arc::new(Catalog { languages, bundles }))
}

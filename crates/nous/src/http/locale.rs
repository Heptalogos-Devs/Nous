use super::{ApiError, ApiState};
use axum::{
    extract::State,
    http::{Request, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use nous_domain::{CallContext, ConfigScope, Error};
use nous_foundation::localization::{LOCALIZATION, LocalizationPolicy, Localizer, Presentation};

pub(super) async fn policy(
    app: &nous_app::App,
    ctx: &CallContext,
) -> Result<LocalizationPolicy, Error> {
    app.config_view(ctx, &ConfigScope::for_caller(ctx))
        .await?
        .active
        .get(LOCALIZATION)
}
pub(super) async fn language(
    State(state): State<ApiState>,
    mut request: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let language = preference(request.headers());
    let result = async {
        let localizer = Localizer::new(policy(&state.app, &CallContext::admin()).await?)?;
        localizer.select_accept_language(&language)
    }
    .await;
    let presentation = match result {
        Ok(value) => value,
        Err(error) => return ApiError::from(error).into_response(),
    };
    request.extensions_mut().insert(presentation.clone());
    let default_language = presentation.locale().to_owned();
    let mut response = crate::presentation::in_language(presentation, next.run(request)).await;
    if !response.headers().contains_key(header::CONTENT_LANGUAGE)
        && let Ok(locale) = default_language.parse()
    {
        response
            .headers_mut()
            .insert(header::CONTENT_LANGUAGE, locale);
    }
    response.headers_mut().append(
        header::VARY,
        header::HeaderValue::from_static("Accept-Language"),
    );
    response
}
pub(super) async fn authenticated(
    app: &nous_app::App,
    ctx: &CallContext,
    language: &str,
) -> Result<Presentation, Error> {
    let localizer = Localizer::new(policy(app, ctx).await?)?;
    localizer.select_accept_language(language)
}
pub(super) fn preference(headers: &axum::http::HeaderMap) -> String {
    headers
        .get(header::ACCEPT_LANGUAGE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_owned()
}

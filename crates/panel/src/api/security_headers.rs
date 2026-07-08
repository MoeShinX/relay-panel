//! Security response headers applied to every panel response (API + static
//! SPA assets).
//!
//! These are defence-in-depth mitigations for the panel's web UI:
//! - `X-Content-Type-Options: nosniff` — blocks MIME sniffing.
//! - `Referrer-Policy` — limits how much referrer info leaks to third parties.
//! - `X-Frame-Options: DENY` — the panel must never be framed (clickjacking).
//!   Combined with CSP `frame-ancestors 'none'` for modern browsers.
//! - `Content-Security-Policy` — the main XSS mitigation. Only same-origin
//!   scripts; antd v6's CSS-in-JS needs `style-src 'self' 'unsafe-inline'`
//!   (the inline styles are generated at runtime, not attacker-controlled).
//!   `script-src` stays strict `'self'` — Vite's production build has no inline
//!   scripts.
//! - `Permissions-Policy` — disable powerful APIs the panel doesn't use.
//!
//! HSTS is deliberately NOT set here: it must only be sent over HTTPS, and the
//! panel's HTTP listener may sit behind a TLS-terminating reverse proxy (Caddy).
//! HSTS belongs in the proxy layer (see docs/REVERSE-PROXY.md).
//!
//! The header values live here as `pub const` so a test asserts the exact
//! strings. The layer chain is built in `main.rs` via `tower_http`'s
//! `SetResponseHeaderLayer` (one per header), each of which is `Clone+Send+Sync`
//! and composes cleanly onto an axum `Router`.

use axum::http::HeaderValue;

/// The CSP value. See the module docs for the rationale behind each directive.
pub const CSP_VALUE: &str = "default-src 'self'; \
    script-src 'self'; \
    style-src 'self' 'unsafe-inline'; \
    object-src 'none'; \
    base-uri 'self'; \
    frame-ancestors 'none'; \
    form-action 'self'; \
    img-src 'self' data:; \
    connect-src 'self';";

/// `Permissions-Policy`: disable every powerful API the panel does not use.
/// Conservative default — if a feature is ever needed, opt it back in explicitly.
pub const PERMISSIONS_POLICY_VALUE: &str = "accelerometer=(), \
    ambient-light-sensor=(), \
    autoplay=(), \
    battery=(), \
    camera=(), \
    display-capture=(), \
    document-domain=(), \
    encrypted-media=(), \
    fullscreen=(), \
    geolocation=(), \
    gyroscope=(), \
    interest-cohort=(), \
    magnetometer=(), \
    microphone=(), \
    midi=(), \
    payment=(), \
    picture-in-picture=(), \
    publickey-credentials-get=(), \
    screen-wake-lock=(), \
    serial=(), \
    sync-xhr=(), \
    usb=(), \
    web-share=(), \
    xr-spatial-tracking=()";

/// Apply the security-response-header layers to a router. Each header is a
/// `SetResponseHeaderLayer::if_not_present` so a header already set by a
/// reverse proxy (e.g. a stricter CSP at the edge) is preserved, not clobbered.
///
/// Implemented as an extension method on `axum::Router<S>` so the caller
/// (`main.rs`) writes `app.layer(...)` chains without needing to name the
/// stacked-layer type. The header values come from the `pub const`s above, so
/// the tests below (which exercise a router built with the same helper) pin
/// the exact strings.
pub fn apply_security_headers<S>(app: axum::Router<S>) -> axum::Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    app.layer(
        tower_http::set_header::SetResponseHeaderLayer::if_not_present(
            axum::http::header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ),
    )
    .layer(
        tower_http::set_header::SetResponseHeaderLayer::if_not_present(
            axum::http::header::REFERRER_POLICY,
            HeaderValue::from_static("strict-origin-when-cross-origin"),
        ),
    )
    .layer(
        tower_http::set_header::SetResponseHeaderLayer::if_not_present(
            axum::http::HeaderName::from_static("x-frame-options"),
            HeaderValue::from_static("DENY"),
        ),
    )
    .layer(
        tower_http::set_header::SetResponseHeaderLayer::if_not_present(
            axum::http::HeaderName::from_static("content-security-policy"),
            HeaderValue::from_static(CSP_VALUE),
        ),
    )
    .layer(
        tower_http::set_header::SetResponseHeaderLayer::if_not_present(
            axum::http::HeaderName::from_static("permissions-policy"),
            HeaderValue::from_static(PERMISSIONS_POLICY_VALUE),
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    /// Build a tiny router with one route + the security-headers, send a
    /// request, and return the response so tests can assert on headers.
    async fn response_from_layered_router() -> axum::response::Response {
        let app = apply_security_headers(
            axum::Router::new().route("/__probe", axum::routing::get(|| async { "ok" })),
        );
        app.oneshot(
            Request::builder()
                .uri("/__probe")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn sets_x_content_type_options_nosniff() {
        let resp = response_from_layered_router().await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get("x-content-type-options").unwrap(),
            "nosniff"
        );
    }

    #[tokio::test]
    async fn sets_referrer_policy() {
        let resp = response_from_layered_router().await;
        assert_eq!(
            resp.headers().get("referrer-policy").unwrap(),
            "strict-origin-when-cross-origin"
        );
    }

    #[tokio::test]
    async fn sets_x_frame_options_deny() {
        let resp = response_from_layered_router().await;
        assert_eq!(resp.headers().get("x-frame-options").unwrap(), "DENY");
    }

    #[tokio::test]
    async fn sets_content_security_policy() {
        let resp = response_from_layered_router().await;
        let csp = resp.headers().get("content-security-policy").unwrap();
        let csp = csp.to_str().unwrap();
        // The hard requirements from the spec (task PR2 §二):
        assert!(csp.contains("default-src 'self'"), "csp: {csp}");
        assert!(csp.contains("script-src 'self'"), "csp: {csp}");
        assert!(csp.contains("object-src 'none'"), "csp: {csp}");
        assert!(csp.contains("base-uri 'self'"), "csp: {csp}");
        assert!(csp.contains("frame-ancestors 'none'"), "csp: {csp}");
        // script-src must NOT be widened (the whole point of CSP).
        assert!(
            !csp.contains("script-src 'self' 'unsafe-inline'"),
            "script-src must stay strict (no unsafe-inline): {csp}"
        );
        assert!(
            !csp.contains("unsafe-eval"),
            "script-src must stay strict (no unsafe-eval): {csp}"
        );
        // style-src IS widened for antd CSS-in-JS (only style-src).
        assert!(
            csp.contains("style-src 'self' 'unsafe-inline'"),
            "csp: {csp}"
        );
    }

    #[tokio::test]
    async fn sets_permissions_policy_conservative() {
        let resp = response_from_layered_router().await;
        let pp = resp
            .headers()
            .get("permissions-policy")
            .unwrap()
            .to_str()
            .unwrap();
        // The dangerous APIs the panel never uses must be disabled.
        assert!(pp.contains("camera=()"), "pp: {pp}");
        assert!(pp.contains("microphone=()"), "pp: {pp}");
        assert!(pp.contains("geolocation=()"), "pp: {pp}");
        assert!(pp.contains("usb=()"), "pp: {pp}");
    }

    #[tokio::test]
    async fn does_not_set_hsts() {
        // HSTS must be left to the HTTPS/proxy layer — the panel may listen on
        // plain HTTP behind Caddy, and sending HSTS over HTTP is ignored by
        // browsers anyway (and risky if a redirect loop is possible).
        let resp = response_from_layered_router().await;
        assert!(
            resp.headers().get("strict-transport-security").is_none(),
            "HSTS must NOT be set by the panel HTTP listener"
        );
    }
}

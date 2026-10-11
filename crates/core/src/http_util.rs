//! Shared HTTP helpers: errors, principal extraction.
use crate::app::AppRef;
use crate::auth::{client_ip, ip_in, is_loopback, Principal};
use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::{request::Parts, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use std::net::{IpAddr, SocketAddr};

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub message: String,
}

impl ApiError {
    pub fn new(status: StatusCode, m: impl Into<String>) -> Self {
        Self { status, message: m.into() }
    }
    pub fn bad(m: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, m)
    }
    pub fn not_found(m: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, m)
    }
    pub fn unauthorized(m: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, m)
    }
    pub fn forbidden(m: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, m)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(serde_json::json!({ "error": self.message }))).into_response()
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        let m = e.to_string();
        let status = if m.contains("не найден") || m.contains("not found") { StatusCode::NOT_FOUND } else { StatusCode::BAD_REQUEST };
        Self::new(status, format!("{e:#}"))
    }
}

pub type R<T> = Result<T, ApiError>;

pub fn bearer(headers: &HeaderMap, query: Option<&str>) -> Option<String> {
    if let Some(v) = headers.get("authorization").and_then(|v| v.to_str().ok()) {
        if let Some(t) = v.strip_prefix("Bearer ").or_else(|| v.strip_prefix("bearer ")) {
            return Some(t.trim().to_string());
        }
    }
    if let Some(v) = headers.get("x-api-key").and_then(|v| v.to_str().ok()) {
        return Some(v.trim().to_string());
    }
    let q = query?;
    q.split('&').find_map(|kv| kv.strip_prefix("token=").map(|t| t.to_string()))
}

pub struct ReqMeta {
    pub ip: IpAddr,
    pub local: bool,
}

pub fn req_meta(app: &AppRef, parts: &Parts) -> ReqMeta {
    let peer = parts.extensions.get::<ConnectInfo<SocketAddr>>().map(|c| c.0.ip()).unwrap_or(IpAddr::from([127, 0, 0, 1]));
    let xff = parts.headers.get("x-forwarded-for").and_then(|v| v.to_str().ok());
    let forwarded = xff.is_some() || parts.headers.contains_key("forwarded") || parts.headers.contains_key("x-real-ip");
    let s = app.settings.read();
    let ip = client_ip(peer, xff, &s.trusted_proxies);
    // a request coming through a local reverse proxy is NOT local
    ReqMeta { ip, local: is_loopback(peer) && !forwarded }
}

/// Resolve the caller. `router` = router endpoint semantics (open while no API keys exist).
pub fn resolve_principal(app: &AppRef, parts: &Parts, router: bool) -> R<(Principal, ReqMeta)> {
    let meta = req_meta(app, parts);
    let (subnets, local_no_auth) = {
        let s = app.settings.read();
        (s.allowed_subnets.clone(), s.local_no_auth)
    };
    if !meta.local && !subnets.is_empty() && !subnets.iter().any(|c| ip_in(meta.ip, c)) {
        return Err(ApiError::forbidden("доступ с этого адреса запрещён"));
    }
    if let Some(t) = bearer(&parts.headers, parts.uri.query()) {
        if let Some(p) = app.auth.principal_from_bearer(&t) {
            return Ok((p, meta));
        }
        if !(meta.local && local_no_auth) {
            return Err(ApiError::unauthorized("неверный или истёкший токен"));
        }
    }
    if meta.local && local_no_auth {
        return Ok((Principal::Local, meta));
    }
    // router: an API key is required only if at least one key exists; no keys = open router
    if router && !app.auth.has_api_keys() {
        return Ok((Principal::Local, meta));
    }
    Err(ApiError::unauthorized("требуется авторизация"))
}

/// Extractor: caller allowed to manage the launcher.
pub struct Admin(pub Principal, pub IpAddr);

impl FromRequestParts<AppRef> for Admin {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, app: &AppRef) -> Result<Self, Self::Rejection> {
        let (p, meta) = resolve_principal(app, parts, false)?;
        if !p.can_manage() {
            return Err(ApiError::forbidden("у ключа нет прав на управление"));
        }
        Ok(Admin(p, meta.ip))
    }
}

/// Primary LAN IPv4 (for showing connection URLs).
pub fn lan_ip() -> Option<IpAddr> {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("8.8.8.8:80").ok()?;
    s.local_addr().ok().map(|a| a.ip())
}

//! Password login + JWT (access) + rotating refresh tokens, API keys, rate limiting, IP checks.
use crate::store::Store;
use crate::types::{new_id, now, ApiKey, UserSession};
use anyhow::{anyhow, bail, Result};
use argon2::password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use parking_lot::Mutex;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::net::IpAddr;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub sid: String,
    pub exp: i64,
    pub iat: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TokenPair {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: i64,
    pub session_id: String,
}

#[derive(Debug, Clone)]
pub enum Principal {
    Local,
    Admin { sid: String },
    Key(ApiKey),
}

impl Principal {
    pub fn can_manage(&self) -> bool {
        match self {
            Principal::Local | Principal::Admin { .. } => true,
            Principal::Key(k) => k.can_manage,
        }
    }
    pub fn can_load(&self) -> bool {
        match self {
            Principal::Key(k) => k.can_load || k.can_manage,
            _ => true,
        }
    }
    pub fn allows_alias(&self, alias: &str) -> bool {
        match self {
            Principal::Key(k) => k.scopes.iter().any(|s| s == "*" || s == alias),
            _ => true,
        }
    }
    /// (api key id, display name) for usage analytics; id is "" without a key
    pub fn user(&self) -> (String, String) {
        match self {
            Principal::Local => (String::new(), "local".into()),
            Principal::Admin { .. } => (String::new(), "admin".into()),
            Principal::Key(k) => (k.id.clone(), k.name.clone()),
        }
    }
    pub fn label(&self) -> String {
        match self {
            Principal::Local => "local".into(),
            Principal::Admin { .. } => "admin".into(),
            Principal::Key(k) => format!("key:{}", k.name),
        }
    }
}

pub fn sha256_hex(s: &str) -> String {
    hex::encode(Sha256::digest(s.as_bytes()))
}

fn random_hex(n: usize) -> String {
    let mut b = vec![0u8; n];
    rand::thread_rng().fill_bytes(&mut b);
    hex::encode(b)
}

pub struct Auth {
    store: Store,
    secret: Vec<u8>,
    fails: Mutex<HashMap<IpAddr, (u32, i64)>>,
}

const MAX_FAILS: u32 = 5;
const LOCK_SECS: i64 = 300;

impl Auth {
    pub fn new(store: Store) -> Result<Self> {
        let secret = match store.kv_get::<String>("auth.jwt_secret")? {
            Some(s) => hex::decode(s)?,
            None => {
                let s = random_hex(48);
                store.kv_set("auth.jwt_secret", &s)?;
                hex::decode(s)?
            }
        };
        Ok(Self { store, secret, fails: Mutex::new(HashMap::new()) })
    }

    pub fn has_password(&self) -> bool {
        self.store.kv_get::<String>("auth.password_hash").ok().flatten().is_some()
    }

    pub fn set_password(&self, pw: &str) -> Result<()> {
        if pw.chars().count() < 8 {
            bail!("пароль должен быть не короче 8 символов");
        }
        let salt = SaltString::generate(&mut OsRng);
        let h = Argon2::default().hash_password(pw.as_bytes(), &salt).map_err(|e| anyhow!("{e}"))?.to_string();
        self.store.kv_set("auth.password_hash", &h)?;
        // revoke all sessions on password change
        for s in self.store.list::<UserSession>("sessions")? {
            self.store.delete("sessions", &s.id)?;
        }
        self.log("password_changed", "", "");
        Ok(())
    }

    fn verify_password(&self, pw: &str) -> bool {
        let Some(h) = self.store.kv_get::<String>("auth.password_hash").ok().flatten() else { return false };
        let Ok(parsed) = PasswordHash::new(&h) else { return false };
        Argon2::default().verify_password(pw.as_bytes(), &parsed).is_ok()
    }

    pub fn log(&self, event: &str, ip: &str, detail: &str) {
        let _ = self.store.log_append("security_log", &serde_json::json!({"event": event, "ip": ip, "detail": detail}), 2000);
    }

    pub fn locked_for(&self, ip: IpAddr) -> Option<i64> {
        let f = self.fails.lock();
        f.get(&ip).and_then(|(n, until)| (*n >= MAX_FAILS && *until > now()).then(|| until - now()))
    }

    pub fn login(&self, pw: &str, ip: IpAddr, device: &str, access_ttl: i64, refresh_days: i64) -> Result<TokenPair> {
        if let Some(s) = self.locked_for(ip) {
            bail!("слишком много попыток, повторите через {s} с");
        }
        if !self.has_password() {
            bail!("пароль не задан — задайте его в настройках на этом ПК");
        }
        if !self.verify_password(pw) {
            let mut f = self.fails.lock();
            let e = f.entry(ip).or_insert((0, 0));
            if e.1 < now() && e.0 >= MAX_FAILS {
                *e = (0, 0);
            }
            e.0 += 1;
            e.1 = now() + LOCK_SECS;
            drop(f);
            self.log("login_failed", &ip.to_string(), device);
            bail!("неверный пароль");
        }
        self.fails.lock().remove(&ip);
        let refresh = random_hex(32);
        let s = UserSession {
            id: new_id(),
            refresh_hash: sha256_hex(&refresh),
            device: device.chars().take(200).collect(),
            ip: ip.to_string(),
            created_at: now(),
            last_seen: now(),
            expires_at: now() + refresh_days * 86400,
        };
        self.store.put("sessions", &s.id, &s)?;
        self.log("login", &ip.to_string(), device);
        Ok(TokenPair { access_token: self.issue(&s.id, access_ttl)?, refresh_token: refresh, expires_in: access_ttl, session_id: s.id })
    }

    fn issue(&self, sid: &str, ttl: i64) -> Result<String> {
        let c = Claims { sub: "admin".into(), sid: sid.into(), iat: now(), exp: now() + ttl };
        Ok(encode(&Header::default(), &c, &EncodingKey::from_secret(&self.secret))?)
    }

    pub fn refresh(&self, refresh: &str, access_ttl: i64, refresh_days: i64) -> Result<TokenPair> {
        let h = sha256_hex(refresh);
        let mut s = self
            .store
            .list::<UserSession>("sessions")?
            .into_iter()
            .find(|s| s.refresh_hash == h)
            .ok_or_else(|| anyhow!("сессия не найдена"))?;
        if s.expires_at < now() {
            self.store.delete("sessions", &s.id)?;
            bail!("сессия истекла");
        }
        let new_refresh = random_hex(32);
        s.refresh_hash = sha256_hex(&new_refresh);
        s.last_seen = now();
        s.expires_at = now() + refresh_days * 86400;
        self.store.put("sessions", &s.id, &s)?;
        Ok(TokenPair { access_token: self.issue(&s.id, access_ttl)?, refresh_token: new_refresh, expires_in: access_ttl, session_id: s.id })
    }

    pub fn verify_access(&self, token: &str) -> Result<Claims> {
        let d = decode::<Claims>(token, &DecodingKey::from_secret(&self.secret), &Validation::default())?;
        if self.store.get::<UserSession>("sessions", &d.claims.sid)?.is_none() {
            bail!("сессия отозвана");
        }
        Ok(d.claims)
    }

    pub fn logout(&self, sid: &str) -> Result<()> {
        self.store.delete("sessions", sid)?;
        Ok(())
    }

    pub fn sessions(&self) -> Result<Vec<UserSession>> {
        self.store.list("sessions")
    }

    // ---- API keys
    pub fn create_key(&self, name: &str, scopes: Vec<String>, can_load: bool, can_manage: bool, expires_at: Option<i64>) -> Result<(ApiKey, String)> {
        let plain = format!("lll_{}", random_hex(20));
        let k = ApiKey {
            id: new_id(),
            name: name.into(),
            prefix: plain[..10].to_string(),
            hash: sha256_hex(&plain),
            scopes: if scopes.is_empty() { vec!["*".into()] } else { scopes },
            can_load,
            can_manage,
            expires_at,
            last_used: None,
            created_at: now(),
        };
        self.store.put("api_keys", &k.id, &k)?;
        self.log("api_key_created", "", name);
        Ok((k, plain))
    }

    pub fn verify_key(&self, plain: &str) -> Option<ApiKey> {
        let h = sha256_hex(plain);
        let mut k = self.store.list::<ApiKey>("api_keys").ok()?.into_iter().find(|k| k.hash == h)?;
        if k.expires_at.is_some_and(|e| e < now()) {
            return None;
        }
        if k.last_used.map(|t| now() - t > 60).unwrap_or(true) {
            k.last_used = Some(now());
            let _ = self.store.put("api_keys", &k.id, &k);
        }
        Some(k)
    }

    /// Whether at least one valid (non-expired) API key exists. If none, the router is open.
    pub fn has_api_keys(&self) -> bool {
        let n = now();
        self.store.list::<ApiKey>("api_keys").map(|v| v.iter().any(|k| k.expires_at.map_or(true, |e| e > n))).unwrap_or(false)
    }

    /// Resolve bearer token (API key or JWT).
    pub fn principal_from_bearer(&self, token: &str) -> Option<Principal> {
        if token.starts_with("lll_") {
            return self.verify_key(token).map(Principal::Key);
        }
        self.verify_access(token).ok().map(|c| Principal::Admin { sid: c.sid })
    }
}

// ---- IP helpers
pub fn parse_cidr(s: &str) -> Option<(IpAddr, u8)> {
    let (ip, bits) = match s.split_once('/') {
        Some((a, b)) => (a.trim().parse().ok()?, b.trim().parse().ok()?),
        None => {
            let ip: IpAddr = s.trim().parse().ok()?;
            (ip, if ip.is_ipv4() { 32 } else { 128 })
        }
    };
    Some((ip, bits))
}

pub fn ip_in(ip: IpAddr, cidr: &str) -> bool {
    let Some((net, bits)) = parse_cidr(cidr) else { return false };
    let ip = match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
        v4 => v4,
    };
    match (ip, net) {
        (IpAddr::V4(a), IpAddr::V4(b)) => {
            let bits = bits.min(32);
            let mask = if bits == 0 { 0 } else { u32::MAX << (32 - bits) };
            (u32::from(a) & mask) == (u32::from(b) & mask)
        }
        (IpAddr::V6(a), IpAddr::V6(b)) => {
            let bits = bits.min(128);
            let mask = if bits == 0 { 0 } else { u128::MAX << (128 - bits) };
            (u128::from(a) & mask) == (u128::from(b) & mask)
        }
        _ => false,
    }
}

pub fn is_loopback(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => v.is_loopback(),
        IpAddr::V6(v) => v.is_loopback() || v.to_ipv4_mapped().is_some_and(|x| x.is_loopback()),
    }
}

/// Effective client IP honoring X-Forwarded-For from trusted proxies.
pub fn client_ip(peer: IpAddr, xff: Option<&str>, trusted: &[String]) -> IpAddr {
    if trusted.iter().any(|t| ip_in(peer, t)) {
        if let Some(x) = xff {
            if let Some(first) = x.split(',').next().and_then(|s| s.trim().parse().ok()) {
                return first;
            }
        }
    }
    peer
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn flow() {
        let s = Store::memory().unwrap();
        let a = Auth::new(s).unwrap();
        let ip: IpAddr = "192.168.1.5".parse().unwrap();
        assert!(a.login("x", ip, "t", 60, 1).is_err());
        a.set_password("correct horse").unwrap();
        let t = a.login("correct horse", ip, "t", 60, 1).unwrap();
        assert!(a.verify_access(&t.access_token).is_ok());
        let t2 = a.refresh(&t.refresh_token, 60, 1).unwrap();
        assert!(a.refresh(&t.refresh_token, 60, 1).is_err(), "old refresh must be rotated");
        a.logout(&t2.session_id).unwrap();
        assert!(a.verify_access(&t2.access_token).is_err());
        for _ in 0..5 {
            let _ = a.login("bad", ip, "t", 60, 1);
        }
        assert!(a.login("correct horse", ip, "t", 60, 1).is_err(), "locked");
        let (_, plain) = a.create_key("k", vec!["coder".into()], true, false, None).unwrap();
        let p = a.principal_from_bearer(&plain).unwrap();
        assert!(p.allows_alias("coder") && !p.allows_alias("x") && !p.can_manage());
    }
    #[test]
    fn cidr() {
        let ip: IpAddr = "192.168.1.77".parse().unwrap();
        assert!(ip_in(ip, "192.168.1.0/24"));
        assert!(!ip_in(ip, "10.0.0.0/8"));
        assert!(ip_in("::ffff:192.168.1.77".parse().unwrap(), "192.168.0.0/16"));
    }
}

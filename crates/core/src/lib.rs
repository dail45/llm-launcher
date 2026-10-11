pub mod gguf;
pub mod help_parser;
pub mod memory;
pub mod paths;
pub mod store;
pub mod types;
pub mod args;
pub mod backends;
pub mod gpu;
pub mod proc_util;
pub mod models;
pub mod events;
pub mod process;
pub mod auth;
pub mod app;
pub mod bench;
pub mod downloads;
pub mod github;
pub mod hf;
pub mod jobs;
pub mod http_util;
pub mod router;
pub mod api;
pub mod server;

/// Application version (single source: `[workspace.package] version` in Cargo.toml).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Short git commit the binary was built from (may be empty).
pub const GIT_SHA: &str = env!("LLL_GIT_SHA");
/// UTC build date, YYYY-MM-DD.
pub const BUILD_DATE: &str = env!("LLL_BUILD_DATE");

/// Human readable version, e.g. `0.2.0 (1ceba9a, 2025-01-01)`.
pub fn version_string() -> String {
    if GIT_SHA.is_empty() { format!("{VERSION} ({BUILD_DATE})") } else { format!("{VERSION} ({GIT_SHA}, {BUILD_DATE})") }
}

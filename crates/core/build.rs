fn main() {
    let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui/dist");
    if !dist.join("index.html").exists() {
        std::fs::create_dir_all(&dist).ok();
        std::fs::write(dist.join("index.html"), "<!doctype html><title>Llama Launcher</title><p>UI not built. Run <code>npm run build</code> in ui/.</p>").ok();
    }
    println!("cargo:rerun-if-changed=../../ui/dist/index.html");

    // Build metadata: commit hash (from CI env or local git) and build date.
    println!("cargo:rerun-if-env-changed=LLL_GIT_SHA");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    let sha = std::env::var("LLL_GIT_SHA").ok().filter(|s| !s.is_empty()).or_else(|| {
        std::process::Command::new("git").args(["rev-parse", "--short=7", "HEAD"]).output().ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    }).unwrap_or_default();
    let sha: String = sha.chars().take(7).collect();
    println!("cargo:rustc-env=LLL_GIT_SHA={sha}");
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let days = (secs / 86400) as i64;
    // civil-from-days (Howard Hinnant)
    let z = days + 719468; let era = z.div_euclid(146097); let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1; let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    println!("cargo:rustc-env=LLL_BUILD_DATE={y:04}-{m:02}-{d:02}");
}

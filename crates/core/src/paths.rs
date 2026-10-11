use std::path::PathBuf;
use std::sync::OnceLock;

static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Override data dir (tests, CLI flag). Must be called before first use.
pub fn set_data_dir(p: PathBuf) {
    let _ = DATA_DIR.set(p);
}

/// Data dir: portable mode (`portable` file or `data` folder next to exe) or %APPDATA%\LlamaLauncher.
pub fn data_dir() -> PathBuf {
    DATA_DIR
        .get_or_init(|| {
            if let Ok(p) = std::env::var("LLL_DATA_DIR") {
                return PathBuf::from(p);
            }
            if let Ok(exe) = std::env::current_exe() {
                if let Some(dir) = exe.parent() {
                    if dir.join("portable").exists() || dir.join("portable.txt").exists() {
                        return dir.join("data");
                    }
                }
            }
            dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("LlamaLauncher")
        })
        .clone()
}

pub fn logs_dir() -> PathBuf {
    data_dir().join("logs")
}
pub fn db_path() -> PathBuf {
    data_dir().join("launcher.db")
}
pub fn exe_name(base: &str) -> String {
    if cfg!(windows) {
        format!("{base}.exe")
    } else {
        base.to_string()
    }
}

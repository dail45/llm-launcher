//! Headless Llama Launcher core (no window). Useful for servers, services and Task Scheduler.
use anyhow::Result;

fn usage() {
    eprintln!(
        "launcherd — Llama Launcher headless\n\n  --data-dir <DIR>     data directory (default: %APPDATA%\\LlamaLauncher)\n  --set-password <PW>  set UI password and exit\n  --version            print version\n"
    );
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let mut set_pw: Option<String> = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--data-dir" => lll_core::paths::set_data_dir(args.next().expect("--data-dir needs a value").into()),
            "--set-password" => set_pw = args.next(),
            "--version" => {
                println!("{}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--minimized" => {}
            "-h" | "--help" => {
                usage();
                return Ok(());
            }
            other => {
                eprintln!("unknown argument {other}");
                usage();
                std::process::exit(2);
            }
        }
    }
    let _guard = lll_core::server::init_logging();
    if let Some(pw) = set_pw {
        let app = lll_core::app::App::open()?;
        app.auth.set_password(&pw)?;
        println!("password set");
        return Ok(());
    }
    let app = lll_core::server::boot().await?;
    let s = app.settings();
    println!("Llama Launcher {} — UI http://{}:{}  router http://{}:{}/v1", env!("CARGO_PKG_VERSION"), s.ui_bind, s.ui_port, s.router_bind, s.router_port);
    tokio::signal::ctrl_c().await?;
    tracing::info!("shutting down, stopping instances");
    app.shutdown().await;
    Ok(())
}

fn main() {
    let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui/dist");
    if !dist.join("index.html").exists() {
        std::fs::create_dir_all(&dist).ok();
        std::fs::write(dist.join("index.html"), "<!doctype html><title>Llama Launcher</title><p>UI not built. Run <code>npm run build</code> in ui/.</p>").ok();
    }
    println!("cargo:rerun-if-changed=../../ui/dist/index.html");
}

use std::env;
use std::path::Path;

fn main() {
    let profile = env::var("PROFILE").unwrap_or_default();
    if profile != "release" {
        return;
    }

    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
    let index_path = Path::new(&manifest_dir)
        .join("web")
        .join("dist")
        .join("index.html");

    if !index_path.is_file() {
        panic!(
            "Missing web/dist/index.html. Please build the frontend before release builds (e.g., `cd web && bun run build` or `npm run build`)."
        );
    }
}

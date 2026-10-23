//! Makes sure the web UI is built before it is embedded (issue 323).
//!
//! A build of the program does not build the UI: that would make Node a
//! requirement of every `cargo build`, `cargo test` and `cargo clippy`. It
//! checks that the UI is built, and says how to build it if not; with
//! `CAELUMORA_BUILD_UI=1` it builds it itself, as a release build does.
//! Without the feature `web-ui` there is nothing to check.

use std::path::Path;
use std::process::{exit, Command};
use std::env;

fn main() {
    println!("cargo:rerun-if-env-changed=CAELUMORA_BUILD_UI");
    if env::var_os("CARGO_FEATURE_WEB_UI").is_none() {
        return;
    }
    let Some(manifest_dir) = env::var_os("CARGO_MANIFEST_DIR") else {
        fail("CARGO_MANIFEST_DIR is not set");
    };
    let ui = Path::new(&manifest_dir).join("../caelumora-web-ui");
    let dist = ui.join("dist");
    // A new build of the UI is embedded anew.
    println!("cargo:rerun-if-changed={}", dist.display());

    if env::var("CAELUMORA_BUILD_UI").is_ok_and(|v| v == "1") {
        for args in [&["ci"][..], &["run", "build"]] {
            match Command::new("npm").args(args).current_dir(&ui).status() {
                Ok(status) if status.success() => {}
                Ok(status) => fail(&format!("`npm {}` in caelumora-web-ui failed: {status}", args.join(" "))),
                Err(e) => fail(&format!("Cannot run npm to build the web UI: {e}")),
            }
        }
    }
    if !dist.join("index.html").is_file() {
        fail("The web UI is not built. Run `npm ci && npm run build` in caelumora-web-ui, \
            build with CAELUMORA_BUILD_UI=1 to have it built, \
            or build without the feature `web-ui` (`--no-default-features`) to serve the API alone.");
    }
}

#[expect(clippy::print_stderr, reason = "a build script reports its failure on stderr")]
fn fail(message: &str) -> ! {
    eprintln!("error: {message}");
    exit(1);
}

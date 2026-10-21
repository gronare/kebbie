use std::path::{Path, PathBuf};
use std::process::Command;

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git").current_dir(dir).args(args).output().ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).trim().to_string()).filter(|text| !text.is_empty())
}

fn main() {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    println!("cargo:rerun-if-changed=build.rs");
    // A source tree unpacked inside another repo must not take that repo's commit.
    let own = git(&root, &["rev-parse", "--show-toplevel"]).is_some_and(|top| Path::new(&top).canonicalize().ok() == root.canonicalize().ok());
    let built = own.then(|| Some(format!("{} {}", git(&root, &["rev-parse", "--short=7", "HEAD"])?, git(&root, &["log", "-1", "--format=%cs", "HEAD"])?))).flatten();
    println!("cargo:rustc-env=KELPIE_BUILT_FROM={}", built.unwrap_or_default());
    if own {
        let reference = git(&root, &["symbolic-ref", "-q", "HEAD"]);
        for name in ["HEAD", "packed-refs"].into_iter().map(str::to_string).chain(reference) {
            if let Some(path) = git(&root, &["rev-parse", "--git-path", &name]) {
                println!("cargo:rerun-if-changed={}", root.join(path).display());
            }
        }
    }
}

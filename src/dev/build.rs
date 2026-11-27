use std::path::Path;

// The installed binary's own commit, asked of the binary, since a pulled checkout that failed to build runs the old code.
fn installed_built() -> Option<String> {
    let root = crate::lead::install_root().ok()?;
    crate::update_check::binary_version(&root.join("target/release/kebbie"))?.1
}

pub fn installed_holds(repo: &Path, sha: &str) -> bool {
    installed_built().is_some_and(|built| holds_in(repo, &built, sha))
}

// Asked of kebbie's full checkout, never of the install's one-commit clone, which cannot place an older commit.
pub fn holds_in(repo: &Path, built: &str, sha: &str) -> bool {
    let known = |commit: &str| crate::sh::succeeds("git", &["cat-file", "-e", &format!("{commit}^{{commit}}")], Some(repo));
    if !known(built) {
        let _ = crate::sh::run("git", &["fetch", "--quiet", "origin"], Some(repo));
    }
    known(built) && known(sha) && crate::sh::succeeds("git", &["merge-base", "--is-ancestor", sha, built], Some(repo))
}

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[path = "../src/executable.rs"]
mod executable;
#[path = "../src/test_seal.rs"]
mod test_seal;

const SCRIPT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/release-notes.sh");
const RELEASE: &str = include_str!("../scripts/release.sh");
const WORKFLOW: &str = include_str!("../.github/workflows/release.yml");

const NOTES: &str = "# Changes\n\nIntro.\n\n## Unreleased\n\n### What's new\n- The hub window names your devices, and r removes\n  one\n\n### Fixed\n- Update no longer stops on a newer config key\n\n## 0.27.0\n\n### What's new\n- Connect a hub from the board\n\n### Fixed\n";

fn file(name: &str, text: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("kebbie-release-notes-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("CHANGES.md");
    fs::write(&path, text).unwrap();
    path
}

fn notes(args: &[&str], changes: &Path) -> Output {
    Command::new("sh").arg(SCRIPT).args(args).arg(changes).output().unwrap()
}

fn out(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn err(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

#[test]
fn a_release_moves_the_unreleased_entries_under_its_version_and_prints_them_as_the_release_body() {
    let changes = file("move", NOTES);
    let moved = notes(&["--move", "0.28.0"], &changes);
    assert!(moved.status.success(), "{}", err(&moved));
    let text = fs::read_to_string(&changes).unwrap();
    assert!(text.contains("## Unreleased\n\n### What's new\n\n### Fixed\n\n## 0.28.0\n\n### What's new\n- The hub window names your devices"), "{text}");
    let body = notes(&["0.28.0"], &changes);
    assert!(body.status.success(), "{}", err(&body));
    assert_eq!(out(&body), "### What's new\n- The hub window names your devices, and r removes\n  one\n\n### Fixed\n- Update no longer stops on a newer config key\n");
    assert_eq!(out(&notes(&["0.27.0"], &changes)), "### What's new\n- Connect a hub from the board\n", "an empty group leaves no heading");
    let again = notes(&["--move", "0.28.0"], &changes);
    assert!(again.status.success() && fs::read_to_string(&changes).unwrap() == text, "a section already moved is left as it is: {}", err(&again));
}

#[test]
fn a_release_with_nothing_written_is_refused() {
    let changes = file("empty", "# Changes\n\n## Unreleased\n\n### What's new\n\n### Fixed\n\n## 0.27.0\n\n### What's new\n- Connect a hub\n");
    let refused = notes(&["--move", "0.28.0"], &changes);
    assert!(!refused.status.success() && err(&refused).contains("nothing under ## Unreleased") && err(&refused).contains("What's new or ### Fixed"), "{}", err(&refused));
    assert!(!fs::read_to_string(&changes).unwrap().contains("0.28.0"), "nothing moved");
    let missing = notes(&["0.29.0"], &changes);
    assert!(!missing.status.success() && err(&missing).contains("no entries under ## 0.29.0"));
}

#[test]
fn release_sh_moves_the_notes_before_the_commit_and_the_workflow_publishes_them_not_the_commit_subjects() {
    let moved = RELEASE.find("release-notes.sh --move \"$version\"").expect("release.sh moves the notes");
    assert!(moved < RELEASE.find("git commit").unwrap() && RELEASE.contains("git add CHANGES.md"), "the release commit carries CHANGES.md");
    assert!(WORKFLOW.contains("scripts/release-notes.sh \"${GITHUB_REF_NAME#v}\" > release-notes.md") && WORKFLOW.contains("--notes-file ../release-notes.md"));
    assert!(!WORKFLOW.contains("--generate-notes"), "the release body is the handwritten notes");
}

const CLEAN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/clean-builds.sh");

fn write(path: &Path, bytes: usize) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, vec![b'x'; bytes]).unwrap();
}

#[test]
fn cleaning_builds_keeps_only_the_installed_release_build_and_never_follows_a_link() {
    let root = std::env::temp_dir().join(format!("kebbie-clean-builds-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let target = root.join("target");
    write(&target.join("debug/deps/kebbie-1.cgu.o"), 3_000_000);
    write(&target.join("verify/debug/kebbie"), 10);
    write(&target.join("x86_64-apple-darwin/release/kebbie"), 10);
    write(&target.join("release/kebbie"), 10);
    write(&target.join("release/kebbie.origin"), 1);
    write(&target.join("CACHEDIR.TAG"), 1);
    write(&root.join("outside/precious"), 1);
    std::os::unix::fs::symlink(root.join("outside"), target.join("linked")).unwrap();
    let cleaned = Command::new("sh").arg(CLEAN).arg(&root).output().unwrap();
    assert!(cleaned.status.success() && out(&cleaned).starts_with("freed ") && out(&cleaned).contains("target/release stays"), "{}{}", out(&cleaned), err(&cleaned));
    for gone in ["debug", "verify", "x86_64-apple-darwin", "linked"] {
        assert!(fs::symlink_metadata(target.join(gone)).is_err(), "{gone} is left");
    }
    for kept in ["release/kebbie", "release/kebbie.origin", "CACHEDIR.TAG"] {
        assert!(target.join(kept).exists(), "{kept} is removed");
    }
    assert!(root.join("outside/precious").exists(), "a link's target is never removed");
    fs::remove_dir_all(&target).unwrap();
    std::os::unix::fs::symlink(root.join("outside"), &target).unwrap();
    assert!(Command::new("sh").arg(CLEAN).arg(&root).status().unwrap().success());
    assert!(root.join("outside/precious").exists(), "a linked target/ is left alone");
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn release_sh_cleans_the_build_output_once_the_release_is_published_unless_told_to_keep_it() {
    let published = RELEASE.find("is published:").expect("release.sh says when the release is published");
    assert!(RELEASE.find("scripts/clean-builds.sh").is_some_and(|cleaned| cleaned > published), "the build output goes only after the release is out");
    assert!(RELEASE.contains("\"${KEBBIE_KEEP_BUILDS:-}\" = 1"));
}

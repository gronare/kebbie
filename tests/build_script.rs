use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[path = "../src/executable.rs"]
mod executable;
#[path = "../src/test_seal.rs"]
mod test_seal;

const SCRIPT: &str = include_str!("../scripts/build.sh");
const PREBUILT: &str = "#!/bin/sh\necho prebuilt\n";

fn triple() -> String {
    let os = if cfg!(target_os = "macos") { "apple-darwin" } else { "unknown-linux-gnu" };
    format!("{}-{os}", std::env::consts::ARCH)
}

fn git(cwd: &Path, args: &[&str]) -> String {
    let output = Command::new("git").args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false", "-c", "core.hooksPath=/dev/null", "-c", "init.defaultBranch=main"]).args(args).current_dir(cwd).output().unwrap();
    assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn executable(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    crate::executable::write(path, text).unwrap();
}

struct Lab {
    dir: PathBuf,
    origin: PathBuf,
    root: PathBuf,
    releases: PathBuf,
    bin: PathBuf,
    cargo_log: PathBuf,
}

impl Lab {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().canonicalize().unwrap().join(format!("kebbie-build-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let origin = dir.join("origin");
        fs::create_dir_all(origin.join("src")).unwrap();
        executable(&origin.join("scripts/build.sh"), SCRIPT);
        fs::write(origin.join("Cargo.toml"), "[package]\nname = \"kebbie\"\nversion = \"1.2.3\"\n").unwrap();
        fs::write(origin.join("src/main.rs"), "fn main() {}\n").unwrap();
        git(&origin, &["init", "--quiet"]);
        git(&origin, &["add", "-A"]);
        git(&origin, &["commit", "--quiet", "-m", "kebbie 1.2.3"]);
        git(&origin, &["tag", "-a", "-m", "kebbie 1.2.3", "v1.2.3"]);
        let bin = dir.join("bin");
        fs::create_dir_all(&bin).unwrap();
        let lab = Lab { root: dir.join("root"), releases: dir.join("releases"), cargo_log: dir.join("cargo.log"), dir, origin, bin };
        lab.herdr_checkout("v1.2.3");
        lab
    }

    fn herdr_checkout(&self, reference: &str) {
        let _ = fs::remove_dir_all(&self.root);
        fs::create_dir_all(&self.root).unwrap();
        git(&self.root, &["init", "--quiet"]);
        git(&self.root, &["remote", "add", "origin", &format!("file://{}", self.origin.display())]);
        git(&self.root, &["fetch", "--quiet", "--depth", "1", "origin", reference]);
        git(&self.root, &["checkout", "--quiet", "--detach", "FETCH_HEAD"]);
        assert_eq!(git(&self.root, &["tag", "--points-at", "HEAD"]), "", "a herdr checkout has no local tags");
    }

    fn publish(&self, binary: &str, sums: Option<&str>) {
        let release = self.releases.join("v1.2.3");
        let stage = self.dir.join("stage");
        fs::create_dir_all(&release).unwrap();
        executable(&stage.join("kebbie"), binary);
        let asset = format!("kebbie-1.2.3-{}.tar.gz", triple());
        let packed = Command::new("tar").args(["-czf", &release.join(&asset).to_string_lossy(), "-C", &stage.to_string_lossy(), "kebbie"]).status().unwrap();
        assert!(packed.success());
        let sum = Command::new("shasum").args(["-a", "256", &asset]).current_dir(&release).output().unwrap();
        let line = String::from_utf8_lossy(&sum.stdout).to_string();
        fs::write(release.join("SHA256SUMS"), sums.map_or(line, str::to_string)).unwrap();
    }

    fn fake_cargo(&self) {
        executable(&self.bin.join("cargo"), &format!("#!/bin/sh\necho \"$*\" >> '{}'\nmkdir -p target/release && printf '#!/bin/sh\\necho built\\n' > target/release/kebbie && chmod 755 target/release/kebbie\n", self.cargo_log.display()));
    }

    fn build(&self, channel: Option<&str>) -> Output {
        self.build_reporting(channel, None)
    }

    fn build_reporting(&self, channel: Option<&str>, progress: Option<&Path>) -> Output {
        let path = format!("{}:/usr/bin:/bin:/usr/sbin:/sbin", self.bin.display());
        let mut command = Command::new("sh");
        command.arg("scripts/build.sh").current_dir(&self.root).env_clear().env("PATH", path).env("HOME", self.dir.join("home")).env("KEBBIE_RELEASES_URL", format!("file://{}", self.releases.display()));
        if let Some(channel) = channel {
            command.env("KEBBIE_CHANNEL", channel);
        }
        if let Some(progress) = progress {
            command.env("KEBBIE_PROGRESS_DIR", progress);
        }
        command.output().unwrap()
    }

    // An edit inside the file system's timestamp tick after the last build is not newer to build.sh's find -newer, so it is dated past the binary.
    fn edit_source(&self, text: &str) {
        let file = self.root.join("src/main.rs");
        fs::write(&file, text).unwrap();
        let built = fs::metadata(self.root.join("target/release/kebbie")).and_then(|binary| binary.modified()).unwrap_or_else(|_| std::time::SystemTime::now());
        fs::File::options().write(true).open(&file).unwrap().set_modified(built + std::time::Duration::from_secs(2)).unwrap();
    }

    fn binary(&self) -> String {
        fs::read_to_string(self.root.join("target/release/kebbie")).unwrap_or_default()
    }

    fn origin_file(&self) -> String {
        fs::read_to_string(self.root.join("target/release/kebbie.origin")).unwrap_or_default()
    }

    fn cargo_runs(&self) -> usize {
        fs::read_to_string(&self.cargo_log).unwrap_or_default().lines().count()
    }
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

#[test]
fn a_release_tag_installs_the_published_binary_without_cargo_and_keeps_it_while_fresh() {
    let lab = Lab::new("download");
    lab.publish(PREBUILT, None);
    let output = lab.build(None);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(lab.binary(), PREBUILT);
    assert_eq!(lab.origin_file(), format!("downloaded v1.2.3 {}\n", triple()));
    assert!(fs::read_dir(lab.root.join("target")).unwrap().all(|entry| !entry.unwrap().file_name().to_string_lossy().starts_with(".kebbie-download")), "the download's temp folder is gone");
    fs::remove_dir_all(&lab.releases).unwrap();
    let again = lab.build(None);
    assert!(again.status.success() && stderr(&again).is_empty(), "a binary newer than every source is kept: {}", stderr(&again));
}

#[test]
fn a_download_that_fails_its_checksum_is_not_used_and_nothing_is_built() {
    let lab = Lab::new("checksum");
    lab.fake_cargo();
    lab.publish(PREBUILT, Some(&format!("{}  kebbie-1.2.3-{}.tar.gz\n", "0".repeat(64), triple())));
    let refused = lab.build(None);
    assert!(!refused.status.success());
    let said = stderr(&refused);
    assert!(said.contains("does not match SHA256SUMS") && said.contains("the release binary of kebbie 1.2.3 was not installed"), "{said}");
    assert_eq!((lab.binary().as_str(), lab.cargo_runs()), ("", 0), "nothing is placed and a release tag is never built");
    lab.publish(PREBUILT, None);
    let fixed = lab.build(None);
    assert!(fixed.status.success(), "the next try downloads again: {}", stderr(&fixed));
    assert_eq!(lab.binary(), PREBUILT);
}

#[test]
fn herdrs_own_build_of_an_unpublished_tag_stops_unless_the_machine_follows_main() {
    let lab = Lab::new("herdr-draft");
    lab.fake_cargo();
    let draft = lab.build(None);
    assert!(!draft.status.success() && stderr(&draft).contains("kebbie 1.2.3 is not published yet; try again in a while"), "{}", stderr(&draft));
    assert_eq!(lab.cargo_runs(), 0, "herdr plugin install --ref of a draft tag is not built from source");
    let config = lab.dir.join("home/.config/kebbie/config.toml");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    for (written, runs) in [("[board]\nchannel = \"release\"\n\n[update]\nchannel = \"main\"  # follow pushes\n", 1), ("[update] # this machine\nchannel = 'main'\n", 2)] {
        fs::write(&config, written).unwrap();
        let _ = fs::remove_file(lab.root.join("target/release/kebbie"));
        let main = lab.build(None);
        assert!(main.status.success() && stderr(&main).contains("the main channel and --from build from source"), "{written}: {}", stderr(&main));
        assert_eq!(lab.cargo_runs(), runs, "a machine on main builds the tag from source: {written}");
    }
    fs::write(&config, "[update]\nchannel = \"release\"\n").unwrap();
    let _ = fs::remove_file(lab.root.join("target/release/kebbie"));
    let home = lab.dir.join("home");
    let path = format!("{}:/usr/bin:/bin:/usr/sbin:/sbin", lab.bin.display());
    let no_github = Command::new("sh").arg("scripts/build.sh").current_dir(&lab.root).env_clear().env("PATH", path).env("HOME", &home).output().unwrap();
    assert!(!no_github.status.success() && stderr(&no_github).contains("kebbie 1.2.3 is a release, which installs only its published binary, and origin is not on GitHub"), "{}", stderr(&no_github));
    assert_eq!(lab.cargo_runs(), 2, "a release tag with no binary to fetch is not built either");
}

#[test]
fn a_commit_past_the_tag_or_the_main_channel_builds_from_source() {
    let lab = Lab::new("source");
    lab.publish(PREBUILT, None);
    lab.fake_cargo();
    let main = lab.build(Some("main"));
    assert!(main.status.success() && stderr(&main).contains("the main channel and --from build from source"), "{}", stderr(&main));
    assert_eq!(lab.cargo_runs(), 1);
    fs::write(lab.origin.join("src/main.rs"), "fn main() { println!(); }\n").unwrap();
    git(&lab.origin, &["commit", "--quiet", "-am", "after the release"]);
    lab.herdr_checkout("main");
    let refused = lab.build(Some("release"));
    assert!(!refused.status.success() && stderr(&refused).contains("the release channel installs only published binaries, and the checkout is not at v1.2.3"), "{}", stderr(&refused));
    assert_eq!(lab.cargo_runs(), 1, "the release channel never builds");
    let past = lab.build(None);
    assert!(past.status.success() && stderr(&past).contains("the checkout is not at v1.2.3"), "{}", stderr(&past));
    assert_eq!((lab.cargo_runs(), lab.binary().as_str()), (2, "#!/bin/sh\necho built\n"));
    fs::remove_file(lab.bin.join("cargo")).unwrap();
    lab.edit_source("fn main() { println!(\"edited\"); }\n");
    let missing = lab.build(None);
    assert!(!missing.status.success() && stderr(&missing).contains("cargo is not installed"), "{}", stderr(&missing));
}

#[test]
fn the_release_channel_waits_for_an_unpublished_release_and_never_builds_it() {
    let lab = Lab::new("unpublished");
    lab.fake_cargo();
    for attempt in 0..2 {
        let draft = lab.build(Some("release"));
        assert!(!draft.status.success(), "attempt {attempt}");
        let said = stderr(&draft);
        assert!(said.contains("kebbie 1.2.3 is not published yet; try again in a while") && !said.contains("building from source"), "attempt {attempt}: {said}");
    }
    assert_eq!((lab.cargo_runs(), lab.binary().as_str()), (0, ""), "a draft release is never built from source");
    lab.publish(PREBUILT, None);
    let published = lab.build(Some("release"));
    assert!(published.status.success(), "a failed download does not hold the next try back: {}", stderr(&published));
    assert_eq!(lab.binary(), PREBUILT);
}

#[test]
fn a_source_build_without_rustup_refuses_a_rustc_older_than_the_workspace_needs() {
    let lab = Lab::new("toolchain");
    fs::write(lab.origin.join("Cargo.toml"), "[package]\nname = \"kebbie\"\nversion = \"1.2.4\"\nrust-version = \"1.90\"\n").unwrap();
    git(&lab.origin, &["commit", "--quiet", "-am", "kebbie 1.2.4"]);
    lab.herdr_checkout("main");
    lab.fake_cargo();
    executable(&lab.bin.join("rustc"), "#!/bin/sh\necho 'rustc 1.89.0 (29483883e 2025-08-04)'\n");
    let old = lab.build(Some("main"));
    assert!(!old.status.success() && stderr(&old).contains("Rust 1.89 is too old to build kebbie (it needs 1.90): install rustup, or switch to the release channel"), "{}", stderr(&old));
    assert_eq!(lab.cargo_runs(), 0, "cargo never starts on a toolchain it cannot build with");
    executable(&lab.bin.join("rustup"), "#!/bin/sh\n");
    let managed = lab.build(Some("main"));
    assert!(managed.status.success(), "rustup installs the pinned toolchain itself: {}", stderr(&managed));
    fs::remove_file(lab.bin.join("rustup")).unwrap();
    executable(&lab.bin.join("rustc"), "#!/bin/sh\necho 'rustc 1.90.0-nightly (abc 2025-09-01)'\n");
    lab.edit_source("fn main() { println!(\"again\"); }\n");
    let new_enough = lab.build(Some("main"));
    assert!(new_enough.status.success(), "{}", stderr(&new_enough));
    assert_eq!(lab.cargo_runs(), 2);
}

#[test]
fn the_workspace_declares_the_rust_version_it_needs() {
    let manifest: toml::Table = toml::from_str(include_str!("../Cargo.toml")).unwrap();
    let needed = manifest["package"]["rust-version"].as_str().expect("Cargo.toml names rust-version, which scripts/build.sh checks without rustup");
    let pinned: toml::Table = toml::from_str(include_str!("../rust-toolchain.toml")).unwrap();
    let number = |version: &str| version.split('.').map(|part| part.parse::<u64>().unwrap()).collect::<Vec<_>>();
    assert!(number(needed) <= number(pinned["toolchain"]["channel"].as_str().unwrap()), "the pinned toolchain builds what rust-version promises");
}

#[test]
fn with_a_progress_folder_cargo_reports_into_it_and_a_download_names_its_size_and_file() {
    let lab = Lab::new("progress");
    let progress = lab.dir.join("progress");
    fs::create_dir_all(&progress).unwrap();
    executable(&lab.bin.join("cargo"), &format!("#!/bin/sh\necho \"$*\" >> '{}'\necho '{{\"reason\":\"compiler-artifact\"}}'\necho '   Compiling kebbie v1.2.3' >&2\nmkdir -p target/release && printf '#!/bin/sh\\necho built\\n' > target/release/kebbie && chmod 755 target/release/kebbie\n", lab.cargo_log.display()));
    let built = lab.build_reporting(Some("main"), Some(&progress));
    assert!(built.status.success(), "{}", stderr(&built));
    assert_eq!(fs::read_to_string(&lab.cargo_log).unwrap().trim(), "build --release --message-format=json-render-diagnostics", "cargo's JSON goes to the update, so its quiet flag does not");
    assert_eq!(fs::read_to_string(progress.join("cargo.jsonl")).unwrap(), "{\"reason\":\"compiler-artifact\"}\n");
    assert!(fs::read_to_string(progress.join("cargo.log")).unwrap().contains("Compiling kebbie v1.2.3") && !stderr(&built).contains("Compiling"), "{}", stderr(&built));
    executable(&lab.bin.join("cargo"), "#!/bin/sh\necho 'error[E0425]: cannot find value `x`' >&2\nexit 101\n");
    lab.edit_source("fn main() { x }\n");
    let failed = lab.build_reporting(Some("main"), Some(&progress));
    assert!(!failed.status.success() && stderr(&failed).contains("error[E0425]: cannot find value `x`"), "a failed build still says why: {}", stderr(&failed));

    let lab = Lab::new("progress-download");
    lab.publish(PREBUILT, None);
    let downloaded = lab.build_reporting(None, Some(&progress));
    assert!(downloaded.status.success(), "{}", stderr(&downloaded));
    let said = fs::read_to_string(progress.join("download")).unwrap();
    let words: Vec<&str> = said.split_whitespace().collect();
    let size = fs::metadata(lab.releases.join("v1.2.3").join(format!("kebbie-1.2.3-{}.tar.gz", triple()))).unwrap().len();
    assert_eq!((words[0], words[1].parse::<u64>().unwrap()), ("v1.2.3", size), "{said}");
    assert!(words[2].ends_with(&format!("kebbie-1.2.3-{}.tar.gz", triple())), "{said}");
    assert_eq!(lab.binary(), PREBUILT);
}

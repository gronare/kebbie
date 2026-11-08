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

// The runner's own gh, /usr/bin/gh on GitHub's Linux images, must never answer for the fake one or reach GitHub.
fn system_path() -> &'static str {
    static PATH: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    PATH.get_or_init(|| {
        let dir = std::env::temp_dir().canonicalize().unwrap().join(format!("kebbie-build-system-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        for system in ["/usr/bin", "/bin", "/usr/sbin", "/sbin"] {
            for entry in fs::read_dir(system).into_iter().flatten().flatten() {
                let name = entry.file_name();
                if name != "gh" && !dir.join(&name).exists() {
                    let _ = std::os::unix::fs::symlink(entry.path(), dir.join(&name));
                }
            }
        }
        dir.display().to_string()
    })
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
        let usual = "gh_dirs=\"/opt/homebrew/bin /usr/local/bin $HOME/.local/bin\"";
        assert!(SCRIPT.contains(usual), "build.sh names where gh is usually installed");
        executable(&origin.join("scripts/build.sh"), &SCRIPT.replace(usual, &format!("gh_dirs=\"{0}/brew {0}/usr-local $HOME/.local/bin\"", dir.display())));
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
        self.publish_as("1.2.3", binary, sums);
    }

    fn publish_as(&self, version: &str, binary: &str, sums: Option<&str>) {
        let release = self.releases.join(format!("v{version}"));
        let stage = self.dir.join("stage");
        fs::create_dir_all(&release).unwrap();
        executable(&stage.join("kebbie"), binary);
        let asset = format!("kebbie-{version}-{}.tar.gz", triple());
        let packed = Command::new("tar").args(["-czf", &release.join(&asset).to_string_lossy(), "-C", &stage.to_string_lossy(), "kebbie"]).status().unwrap();
        assert!(packed.success());
        let sum = Command::new("shasum").args(["-a", "256", &asset]).current_dir(&release).output().unwrap();
        let line = String::from_utf8_lossy(&sum.stdout).to_string();
        fs::write(release.join("SHA256SUMS"), sums.map_or(line, str::to_string)).unwrap();
    }

    // exit 4 is gh's own code for a command that needs a login.
    fn fake_gh(&self, verify_exit: i32) {
        self.fake_gh_in(&self.bin, verify_exit);
    }

    fn fake_gh_in(&self, dir: &Path, verify_exit: i32) {
        executable(&dir.join("gh"), &format!("#!/bin/sh\n[ \"$1 $2\" = 'attestation verify' ] || exit 1\n[ \"$3\" = --help ] && exit 0\necho \"$*\" >> '{}'\n[ {verify_exit} -eq 1 ] && echo 'Error: verifying with issuer \"sigstore.dev\": failed to verify signature' >&2\nexit {verify_exit}\n", self.gh_log().display()));
    }

    fn gh_log(&self) -> PathBuf {
        self.dir.join("gh.log")
    }

    fn gh_runs(&self) -> Vec<String> {
        fs::read_to_string(self.gh_log()).unwrap_or_default().lines().map(str::to_string).collect()
    }

    fn fake_cargo(&self) {
        executable(&self.bin.join("cargo"), &format!("#!/bin/sh\necho \"$*\" >> '{}'\nmkdir -p target/release && printf '#!/bin/sh\\necho built\\n' > target/release/kebbie && chmod 755 target/release/kebbie\n", self.cargo_log.display()));
    }

    fn build(&self, channel: Option<&str>) -> Output {
        self.build_reporting(channel, None)
    }

    fn build_reporting(&self, channel: Option<&str>, progress: Option<&Path>) -> Output {
        let path = format!("{}:{}", self.bin.display(), system_path());
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
    assert_eq!(lab.origin_file(), format!("downloaded v1.2.3 {} unverified\n", triple()));
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
fn a_download_gh_verifies_is_installed_as_verified_against_the_release_workflow_at_its_tag() {
    let lab = Lab::new("attested");
    lab.fake_gh(0);
    lab.publish(PREBUILT, None);
    let output = lab.build(None);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(!stderr(&output).contains("not verified"), "{}", stderr(&output));
    assert_eq!(lab.binary(), PREBUILT);
    assert_eq!(lab.origin_file(), format!("downloaded v1.2.3 {} verified\n", triple()));
    let runs = lab.gh_runs();
    assert_eq!(runs.len(), 1, "{runs:?}");
    let asset = format!("kebbie-1.2.3-{}.tar.gz ", triple());
    assert!(runs[0].starts_with("attestation verify ") && runs[0].contains(&asset), "gh checks the tarball SHA256SUMS vouched for: {}", runs[0]);
    assert!(runs[0].ends_with("--repo gronare/kebbie --signer-workflow gronare/kebbie/.github/workflows/release.yml --source-ref refs/tags/v1.2.3 --deny-self-hosted-runners"), "{}", runs[0]);
}

#[test]
fn gh_outside_path_is_found_where_it_is_usually_installed_first_one_first() {
    let lab = Lab::new("gh-off-path");
    lab.fake_gh_in(&lab.dir.join("home/.local/bin"), 1);
    lab.publish(PREBUILT, None);
    let refused = lab.build(None);
    assert!(!refused.status.success() && stderr(&refused).contains("failed gh attestation verify"), "a gh in ~/.local/bin runs though PATH lacks it: {}", stderr(&refused));
    lab.fake_gh_in(&lab.dir.join("usr-local"), 0);
    let output = lab.build(None);
    assert!(output.status.success(), "the /usr/local/bin one comes before ~/.local/bin: {}", stderr(&output));
    assert_eq!(lab.origin_file(), format!("downloaded v1.2.3 {} verified\n", triple()));
    assert_eq!(lab.gh_runs().len(), 2);
}

#[test]
fn a_download_that_fails_gh_attestation_verify_is_refused_and_nothing_is_built() {
    let lab = Lab::new("attest-fails");
    lab.fake_cargo();
    lab.fake_gh(1);
    lab.publish(PREBUILT, None);
    let refused = lab.build(None);
    assert!(!refused.status.success());
    let said = stderr(&refused);
    assert!(said.contains("failed gh attestation verify against gronare/kebbie/.github/workflows/release.yml at v1.2.3, so it is not used: Error: verifying with issuer"), "the line names gh's own reason: {said}");
    assert!(said.contains("the release binary of kebbie 1.2.3 was not installed because its provenance check failed"), "{said}");
    assert_eq!((lab.binary().as_str(), lab.origin_file().as_str(), lab.cargo_runs()), ("", "", 0), "nothing is placed and nothing is built in its stead");
    let again = lab.build(Some("release"));
    assert!(!again.status.success() && lab.binary().is_empty(), "the release channel refuses it the same way: {}", stderr(&again));
}

#[test]
fn without_gh_or_its_login_a_download_installs_with_one_warning_and_reads_unverified() {
    for (name, gh) in [("no-gh", None), ("gh-logged-out", Some(4)), ("gh-without-attestation", Some(-1))] {
        let lab = Lab::new(name);
        match gh {
            Some(-1) => executable(&lab.bin.join("gh"), "#!/bin/sh\necho 'unknown command \"attestation\" for \"gh\"' >&2\nexit 1\n"),
            Some(code) => lab.fake_gh(code),
            None => {}
        }
        lab.publish(PREBUILT, None);
        let output = lab.build(None);
        assert!(output.status.success(), "{name}: {}", stderr(&output));
        assert_eq!(lab.binary(), PREBUILT, "{name}");
        assert_eq!(lab.origin_file(), format!("downloaded v1.2.3 {} unverified\n", triple()), "{name}");
        let said = stderr(&output);
        let warnings: Vec<&str> = said.lines().filter(|line| line.contains("was not verified")).collect();
        assert_eq!(warnings.len(), 1, "{name}: {said}");
        let fix = if gh == Some(4) { "gh is not logged in (gh auth login)" } else { "install the GitHub CLI (gh) and log in" };
        assert!(warnings[0].contains(&format!("the origin of kebbie-1.2.3-{}.tar.gz was not verified: {fix}", triple())), "{name}: {said}");
    }
}

#[test]
fn a_release_from_before_attestations_installs_without_asking_gh_and_the_first_attested_one_is_checked() {
    let lab = Lab::new("pre-attest");
    lab.fake_gh(1);
    for version in ["0.36.0", "0.37.0"] {
        fs::write(lab.origin.join("Cargo.toml"), format!("[package]\nname = \"kebbie\"\nversion = \"{version}\"\n")).unwrap();
        git(&lab.origin, &["commit", "--quiet", "-am", &format!("kebbie {version}")]);
        git(&lab.origin, &["tag", "-a", "-m", &format!("kebbie {version}"), &format!("v{version}")]);
        lab.publish_as(version, PREBUILT, None);
    }
    lab.herdr_checkout("v0.36.0");
    let old = lab.build(None);
    assert!(old.status.success() && !stderr(&old).contains("verif"), "{}", stderr(&old));
    assert_eq!((lab.origin_file(), lab.gh_runs().len()), (format!("downloaded v0.36.0 {}\n", triple()), 0), "0.36.0 was published before its tarballs were attested");
    lab.herdr_checkout("v0.37.0");
    let first = lab.build(None);
    assert!(!first.status.success() && stderr(&first).contains("failed gh attestation verify"), "{}", stderr(&first));
    assert_eq!(lab.gh_runs().len(), 1);
}

#[test]
fn a_download_whose_kebbie_is_a_symlink_or_comes_with_other_files_places_only_a_real_binary() {
    let lab = Lab::new("symlink");
    let release = lab.releases.join("v1.2.3");
    let stage = lab.dir.join("stage");
    let asset = format!("kebbie-1.2.3-{}.tar.gz", triple());
    let pack = |names: &[&str]| {
        fs::create_dir_all(&release).unwrap();
        let packed = Command::new("tar").args(["-czf", &release.join(&asset).to_string_lossy(), "-C", &stage.to_string_lossy()]).args(names).status().unwrap();
        assert!(packed.success());
        let sum = Command::new("shasum").args(["-a", "256", &asset]).current_dir(&release).output().unwrap();
        fs::write(release.join("SHA256SUMS"), String::from_utf8_lossy(&sum.stdout).to_string()).unwrap();
    };
    fs::create_dir_all(&stage).unwrap();
    executable(&lab.dir.join("elsewhere"), PREBUILT);
    std::os::unix::fs::symlink(lab.dir.join("elsewhere"), stage.join("kebbie")).unwrap();
    pack(&["kebbie"]);
    let refused = lab.build(None);
    assert!(!refused.status.success() && stderr(&refused).contains("holds no kebbie binary"), "{}", stderr(&refused));
    assert!(fs::symlink_metadata(lab.root.join("target/release/kebbie")).is_err(), "no link is placed as kebbie");
    fs::remove_file(stage.join("kebbie")).unwrap();
    executable(&stage.join("kebbie"), PREBUILT);
    fs::write(stage.join("extra"), "x").unwrap();
    pack(&["kebbie", "extra"]);
    let placed = lab.build(None);
    assert!(placed.status.success(), "{}", stderr(&placed));
    assert_eq!(lab.binary(), PREBUILT);
    assert!(!lab.root.join("target/release/extra").exists());
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
    let path = format!("{}:{}", lab.bin.display(), system_path());
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
    git(&lab.origin, &["tag", "v1.2.10"]);
    git(&lab.origin, &["tag", "v1.3.0-rc1"]);
    let named = stderr(&lab.build(None));
    assert!(named.contains("herdr plugin install --ref v1.2.10 gronare/kebbie") && named.contains("kebbie.dev/install.sh"), "the way out names the latest release, by number: {named}");
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

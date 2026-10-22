use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

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
    fs::write(path, text).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
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
        let dir = std::env::temp_dir().canonicalize().unwrap().join(format!("kelpie-build-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let origin = dir.join("origin");
        fs::create_dir_all(origin.join("src")).unwrap();
        executable(&origin.join("scripts/build.sh"), SCRIPT);
        fs::write(origin.join("Cargo.toml"), "[package]\nname = \"kelpie\"\nversion = \"1.2.3\"\n").unwrap();
        fs::write(origin.join("src/main.rs"), "fn main() {}\n").unwrap();
        git(&origin, &["init", "--quiet"]);
        git(&origin, &["add", "-A"]);
        git(&origin, &["commit", "--quiet", "-m", "kelpie 1.2.3"]);
        git(&origin, &["tag", "-a", "-m", "kelpie 1.2.3", "v1.2.3"]);
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
        executable(&stage.join("kelpie"), binary);
        let asset = format!("kelpie-1.2.3-{}.tar.gz", triple());
        let packed = Command::new("tar").args(["-czf", &release.join(&asset).to_string_lossy(), "-C", &stage.to_string_lossy(), "kelpie"]).status().unwrap();
        assert!(packed.success());
        let sum = Command::new("shasum").args(["-a", "256", &asset]).current_dir(&release).output().unwrap();
        let line = String::from_utf8_lossy(&sum.stdout).to_string();
        fs::write(release.join("SHA256SUMS"), sums.map_or(line, str::to_string)).unwrap();
    }

    fn fake_cargo(&self) {
        executable(&self.bin.join("cargo"), &format!("#!/bin/sh\necho \"$*\" >> '{}'\nmkdir -p target/release && printf '#!/bin/sh\\necho built\\n' > target/release/kelpie && chmod 755 target/release/kelpie\n", self.cargo_log.display()));
    }

    fn build(&self, channel: Option<&str>) -> Output {
        let path = format!("{}:/usr/bin:/bin:/usr/sbin:/sbin", self.bin.display());
        let mut command = Command::new("sh");
        command.arg("scripts/build.sh").current_dir(&self.root).env_clear().env("PATH", path).env("HOME", self.dir.join("home")).env("KELPIE_RELEASES_URL", format!("file://{}", self.releases.display()));
        if let Some(channel) = channel {
            command.env("KELPIE_CHANNEL", channel);
        }
        command.output().unwrap()
    }

    fn binary(&self) -> String {
        fs::read_to_string(self.root.join("target/release/kelpie")).unwrap_or_default()
    }

    fn origin_file(&self) -> String {
        fs::read_to_string(self.root.join("target/release/kelpie.origin")).unwrap_or_default()
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
    assert!(fs::read_dir(lab.root.join("target")).unwrap().all(|entry| !entry.unwrap().file_name().to_string_lossy().starts_with(".kelpie-download")), "the download's temp folder is gone");
    fs::remove_dir_all(&lab.releases).unwrap();
    let again = lab.build(None);
    assert!(again.status.success() && stderr(&again).is_empty(), "a binary newer than every source is kept: {}", stderr(&again));
}

#[test]
fn a_download_that_fails_its_checksum_is_not_used_and_not_retried_for_ten_minutes() {
    let lab = Lab::new("checksum");
    lab.publish(PREBUILT, Some(&format!("{}  kelpie-1.2.3-{}.tar.gz\n", "0".repeat(64), triple())));
    let refused = lab.build(None);
    assert!(!refused.status.success(), "no cargo to fall back on");
    let said = stderr(&refused);
    assert!(said.contains("does not match SHA256SUMS") && said.contains("the download failed") && said.contains("sh.rustup.rs") && said.contains("herdr plugin install --ref"), "{said}");
    assert_eq!(lab.binary(), "", "nothing is placed");
    assert!(lab.root.join("target/kelpie-download.failed").exists());
    lab.publish(PREBUILT, None);
    lab.fake_cargo();
    let built = lab.build(None);
    assert!(built.status.success(), "{}", stderr(&built));
    assert!(stderr(&built).contains("a download failed less than 10 minutes ago"), "{}", stderr(&built));
    assert_eq!((lab.binary().as_str(), lab.cargo_runs(), lab.origin_file()), ("#!/bin/sh\necho built\n", 1, String::new()));
}

#[test]
fn a_commit_past_the_tag_or_the_main_channel_builds_from_source() {
    let lab = Lab::new("source");
    lab.publish(PREBUILT, None);
    lab.fake_cargo();
    let main = lab.build(Some("main"));
    assert!(main.status.success() && stderr(&main).contains("the main channel builds from source"), "{}", stderr(&main));
    assert_eq!(lab.cargo_runs(), 1);
    fs::write(lab.origin.join("src/main.rs"), "fn main() { println!(); }\n").unwrap();
    git(&lab.origin, &["commit", "--quiet", "-am", "after the release"]);
    lab.herdr_checkout("main");
    let past = lab.build(Some("release"));
    assert!(past.status.success() && stderr(&past).contains("the checkout is not at v1.2.3"), "{}", stderr(&past));
    assert_eq!((lab.cargo_runs(), lab.binary().as_str()), (2, "#!/bin/sh\necho built\n"));
    fs::remove_file(lab.bin.join("cargo")).unwrap();
    fs::write(lab.root.join("src/main.rs"), "fn main() { println!(\"edited\"); }\n").unwrap();
    let missing = lab.build(None);
    assert!(!missing.status.success() && stderr(&missing).contains("cargo is not installed"), "{}", stderr(&missing));
}

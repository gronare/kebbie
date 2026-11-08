use std::fs;
use std::path::PathBuf;
use std::process::Command;

#[path = "../src/executable.rs"]
mod executable;
#[path = "../src/test_seal.rs"]
mod test_seal;

const SCRIPT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/install.sh");

struct Home {
    dir: PathBuf,
    home: PathBuf,
    root: PathBuf,
}

impl Home {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().canonicalize().unwrap().join(format!("kebbie-install-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let (bin, root, home) = (dir.join("bin"), dir.join("root"), dir.join("home"));
        let tool = |name: &str, body: &str| executable::write(bin.join(name), format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::create_dir_all(&bin).unwrap();
        tool("herdr", "case \"$1\" in --version) echo 'herdr 9.9.9' ;; esac");
        tool("jq", &format!("cat >/dev/null\ncase \"$*\" in *'--arg id kebbie'*) echo 'local {}' ;; esac", root.display()));
        tool("curl", "echo https://github.com/gronare/kebbie/releases/tag/v9.9.9");
        tool("git", "exit 0");
        tool("claude", "exit 1");
        fs::create_dir_all(root.join("target/release")).unwrap();
        executable::write(root.join("target/release/kebbie"), "#!/bin/sh\nexit 0\n").unwrap();
        fs::create_dir_all(&home).unwrap();
        Home { dir, home, root }
    }

    fn install(&self) -> String {
        let output = Command::new("sh").arg(SCRIPT).env_clear().env("HOME", &self.home).env("PATH", format!("{}:/usr/bin:/bin", self.dir.join("bin").display())).env("NO_COLOR", "1").stdin(std::process::Stdio::null()).output().unwrap();
        let said = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        assert!(output.status.success(), "{said}");
        said
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn an_install_links_the_plugin_checkout_as_the_current_kebbie() {
    let home = Home::new("current");
    home.install();
    assert!(home.home.join(".local/share/kebbie/current/target/release/kebbie").exists());
}

#[test]
fn an_install_says_whether_gh_verified_the_binarys_attestation() {
    let home = Home::new("provenance");
    let origin = home.root.join("target/release/kebbie.origin");
    fs::write(&origin, "downloaded v9.9.9 aarch64-apple-darwin verified\n").unwrap();
    assert!(home.install().contains("✓ its attestation verified with gh: built by kebbie's release workflow at v9.9.9"));
    fs::write(&origin, "downloaded v9.9.9 aarch64-apple-darwin unverified\n").unwrap();
    let said = home.install();
    assert_eq!(said.matches("not verified").count(), 1, "{said}");
    assert!(said.contains("! the binary's origin was not verified: install the GitHub CLI (gh) and log in"), "{said}");
    fs::write(&origin, "downloaded v0.36.0 aarch64-apple-darwin\n").unwrap();
    assert!(!home.install().contains("verified"), "a release from before attestations says nothing about it");
}

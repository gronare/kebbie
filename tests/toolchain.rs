#[path = "../src/test_seal.rs"]
mod test_seal;

const TOOLCHAIN: &str = include_str!("../rust-toolchain.toml");
const WORKFLOWS: [(&str, &str); 2] = [("ci.yml", include_str!("../.github/workflows/ci.yml")), ("release.yml", include_str!("../.github/workflows/release.yml"))];

#[test]
fn the_toolchain_is_pinned_to_an_exact_release_with_clippy_and_rustfmt() {
    let file: toml::Table = toml::from_str(TOOLCHAIN).unwrap();
    let toolchain = file["toolchain"].as_table().unwrap();
    let channel = toolchain["channel"].as_str().unwrap();
    let parts: Vec<&str> = channel.split('.').collect();
    assert!(parts.len() == 3 && parts.iter().all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit())), "channel {channel:?} is not an exact x.y.z release");
    let components: Vec<&str> = toolchain["components"].as_array().unwrap().iter().map(|c| c.as_str().unwrap()).collect();
    for wanted in ["clippy", "rustfmt"] {
        assert!(components.contains(&wanted), "rust-toolchain.toml lacks {wanted}");
    }
}

#[test]
fn every_workflow_that_runs_cargo_installs_the_pinned_toolchain() {
    for (name, text) in WORKFLOWS {
        assert!(!text.contains("dtolnay/rust-toolchain"), "{name} installs a toolchain of its own instead of the one rust-toolchain.toml pins");
        for job in jobs(text).iter().filter(|job| job.contains("cargo ")) {
            let install = job.find("rustup toolchain install").unwrap_or_else(|| panic!("{name}: a job runs cargo without installing the pinned toolchain:\n{job}"));
            assert!(job.find("cargo ").unwrap() > install, "{name}: a job runs cargo before installing the pinned toolchain:\n{job}");
        }
    }
}

fn jobs(workflow: &str) -> Vec<String> {
    let mut jobs: Vec<String> = Vec::new();
    for line in workflow.lines() {
        if line.starts_with("  ") && !line.starts_with("   ") {
            jobs.push(String::new());
        }
        if let Some(job) = jobs.last_mut() {
            job.push_str(line);
            job.push('\n');
        }
    }
    jobs
}

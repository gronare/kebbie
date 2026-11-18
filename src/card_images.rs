use std::fs;
use std::path::{Path, PathBuf};

// Where kebbie keeps a card's images, beside the card, so the task's scratchpad can be cleaned away before the user reads it.
pub fn dir(state: &Path, address: &str) -> PathBuf {
    state.join("cards").join(address)
}

pub struct Kept {
    pub text: String,
    pub missing: Vec<String>,
}

fn image_in(line: &str) -> Option<(&str, &str)> {
    line.trim().strip_prefix("![").and_then(|rest| rest.split_once("](")).map(|(alt, rest)| (alt, rest.trim_end_matches(')')))
}

fn resolve(base: &Path, path: &str) -> PathBuf {
    match (path.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => base.join(path),
    }
}

fn free_name(dir: &Path, file: &Path, taken: &[PathBuf]) -> PathBuf {
    let name = file.file_name().map_or_else(|| "image".to_string(), |name| name.to_string_lossy().into_owned());
    (1..).map(|at| dir.join(if at == 1 { name.clone() } else { format!("{at}-{name}") })).find(|copy| !taken.contains(copy)).expect("some name is free")
}

// Each image the card names is copied into its folder in kebbie's state and the line points at the copy; an image already in kebbie's card state stays where it is, and copies the card no longer names go.
pub fn keep(state: &Path, address: &str, base: &Path, text: &str) -> Kept {
    let cards = state.join("cards");
    let _ = fs::create_dir_all(&cards);
    let cards = cards.canonicalize().unwrap_or(cards);
    let own = cards.join(address);
    let mut taken: Vec<PathBuf> = Vec::new();
    let mut missing = Vec::new();
    let lines: Vec<String> = text.lines().map(|line| {
        let Some((alt, path)) = image_in(line) else { return line.to_string() };
        let Some(file) = resolve(base, path).canonicalize().ok().filter(|file| file.is_file()) else {
            missing.push(path.to_string());
            return line.to_string();
        };
        if file.starts_with(&cards) {
            taken.push(file);
            return line.to_string();
        }
        let copy = free_name(&own, &file, &taken);
        match fs::create_dir_all(&own).and_then(|_| fs::copy(&file, &copy)) {
            Ok(_) => {
                taken.push(copy.clone());
                format!("![{alt}]({})", copy.display())
            }
            Err(error) => {
                missing.push(format!("{path} ({error})"));
                line.to_string()
            }
        }
    }).collect();
    prune(&own, &taken);
    Kept { text: lines.join("\n"), missing }
}

fn prune(own: &Path, taken: &[PathBuf]) {
    let Ok(entries) = fs::read_dir(own) else { return };
    for entry in entries.flatten() {
        if !taken.contains(&entry.path()) {
            let _ = fs::remove_file(entry.path());
        }
    }
    let _ = fs::remove_dir(own);
}

pub fn forget(state: &Path, address: &str) {
    let _ = fs::remove_dir_all(dir(state, address));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cards_images_are_copied_beside_it_and_its_lines_point_at_the_copies() {
        let dir = crate::sh::test_dir("card-images");
        let (state, scratch, worktree) = (dir.join("state"), dir.join("scratchpad"), dir.join("wt1.app"));
        for folder in [&scratch, &worktree.join("shots"), &state.join("cards/ios-sim")] {
            fs::create_dir_all(folder).unwrap();
        }
        fs::write(scratch.join("sim.png"), "one").unwrap();
        fs::write(worktree.join("shots/sim.png"), "two").unwrap();
        let kept_by_task = state.join("cards/ios-sim/boot.png");
        fs::write(&kept_by_task, "three").unwrap();
        let text = format!("**The simulator boots on the worker**\n\nBuilt:\n- It boots.\n![booted]({})\n![again](shots/sim.png)\n![boot]({})\n![gone](/nowhere/lost.png)", scratch.join("sim.png").display(), kept_by_task.display());
        let kept = keep(&state, "ios-sim@app", &worktree, &text);
        let own = state.canonicalize().unwrap().join("cards/ios-sim@app");
        let lines: Vec<&str> = kept.text.lines().collect();
        assert_eq!(lines[4], format!("![booted]({})", own.join("sim.png").display()));
        assert_eq!(lines[5], format!("![again]({})", own.join("2-sim.png").display()), "two images of one name each keep their own copy");
        assert_eq!(lines[6], format!("![boot]({})", kept_by_task.display()), "an image already in kebbie's card state stays as it is");
        assert_eq!((lines[7], kept.missing.as_slice()), ("![gone](/nowhere/lost.png)", &["/nowhere/lost.png".to_string()][..]), "a missing image is named, its line kept");
        fs::remove_dir_all(&scratch).unwrap();
        assert_eq!((fs::read_to_string(own.join("sim.png")).unwrap(), fs::read_to_string(own.join("2-sim.png")).unwrap()), ("one".to_string(), "two".to_string()), "the copies outlive the scratchpad");
        let again = keep(&state, "ios-sim@app", &worktree, &format!("Built:\n- It boots.\n![booted]({})", own.join("sim.png").display()));
        assert!(again.missing.is_empty() && own.join("sim.png").exists() && !own.join("2-sim.png").exists(), "a copy the new version no longer names goes");
        forget(&state, "ios-sim@app");
        assert!(!own.exists() && kept_by_task.exists(), "dropping the card removes its copies and nothing the task put there itself");
        fs::remove_dir_all(dir).unwrap();
    }
}

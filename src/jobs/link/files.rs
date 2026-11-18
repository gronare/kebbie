use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

use super::wire::{CHUNK, Frame, Link, MAX_FILES, Said, SendAsk};
use crate::sh::Result;

// What a run sends along and fetches back: local files for the run's folder, paths in that folder, and where fetched ones land.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Files {
    pub send: Vec<PathBuf>,
    pub fetch: Vec<String>,
    pub out: Option<PathBuf>,
}

const LIMIT: &str = "1 GiB";

fn mib(bytes: u64) -> String {
    format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
}

// A path inside the run's folder as written: relative, without .., never the folder itself.
pub fn inside(path: &str) -> Result<PathBuf> {
    let mut kept = PathBuf::new();
    for component in Path::new(path).components() {
        match component {
            Component::Normal(part) => kept.push(part),
            Component::CurDir => {}
            _ => return Err(format!("--fetch {path}: name a file or folder inside the run's folder, by a relative path without ..")),
        }
    }
    if kept.as_os_str().is_empty() {
        return Err(format!("--fetch {path}: name a file or folder inside the run's folder, not the folder itself"));
    }
    Ok(kept)
}

fn send_name(name: &str) -> Result<()> {
    let one = matches!(Path::new(name).components().collect::<Vec<_>>().as_slice(), [Component::Normal(part)] if part.to_str() == Some(name));
    if one { Ok(()) } else { Err(format!("--send names a file by its name alone, not \"{name}\"")) }
}

// The local files to send, each a file under its own name and all of them, as all a run fetches, within MAX_FILES.
pub fn plan(files: &Files) -> Result<Vec<(PathBuf, SendAsk)>> {
    let mut planned: Vec<(PathBuf, SendAsk)> = Vec::new();
    for local in &files.send {
        let meta = std::fs::metadata(local).map_err(|error| format!("--send {}: {error}", local.display()))?;
        if !meta.is_file() {
            return Err(format!("--send {}: is no file; --send takes files one by one", local.display()));
        }
        let name = local.file_name().and_then(|name| name.to_str()).ok_or_else(|| format!("--send {}: its name is no UTF-8", local.display()))?.to_string();
        if planned.iter().any(|(_, ask)| ask.name == name) {
            return Err(format!("--send {}: another file sent is also called {name}, and both would land as {name} in the run's folder", local.display()));
        }
        planned.push((local.clone(), SendAsk { name, size: meta.len(), mode: meta.permissions().mode() & 0o777 }));
    }
    let total: u64 = planned.iter().map(|(_, ask)| ask.size).sum();
    if total > MAX_FILES {
        return Err(format!("the files sent are {}, past the {LIMIT} a run sends and fetches in all", mib(total)));
    }
    for fetch in &files.fetch {
        inside(fetch)?;
    }
    Ok(planned)
}

// What the worker checks before it takes a run's files.
pub fn check(sends: &[SendAsk], fetches: &[String]) -> Result<()> {
    for send in sends {
        send_name(&send.name)?;
    }
    if sends.iter().map(|send| send.size).sum::<u64>() > MAX_FILES {
        return Err(format!("the files sent are past the {LIMIT} a run sends and fetches in all"));
    }
    fetches.iter().try_for_each(|fetch| inside(fetch).map(|_| ()))
}

fn io(error: std::io::Error) -> String {
    error.to_string()
}

// Each planned file as Data frames and an End, in the order the ask named them.
pub fn stream(link: &mut Link, planned: &[(PathBuf, SendAsk)]) -> std::result::Result<(), std::io::Error> {
    let mut buffer = vec![0u8; CHUNK];
    for (local, ask) in planned {
        let mut file = File::open(local)?.take(ask.size);
        loop {
            let read = file.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            link.send(&Frame::Data(buffer[..read].to_vec()))?;
        }
        link.send(&Frame::End)?;
    }
    Ok(())
}

fn temporary(at: &Path) -> PathBuf {
    let name = at.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
    at.with_file_name(format!(".kebbie-{}-{name}", std::process::id()))
}

// One file's Data frames up to its End into a file beside at, renamed over at once whole, so a link at its path is replaced, never written through.
fn receive(link: &mut Link, at: &Path, size: u64, mode: u32, halted: &dyn Fn() -> bool) -> std::result::Result<std::result::Result<(), String>, String> {
    let partial = temporary(at);
    let mut file = at.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).custom_flags(libc::O_NOFOLLOW).open(&partial)).map_err(io);
    let mut got = 0u64;
    loop {
        match link.expect_unless(halted).map_err(io)? {
            Frame::Data(bytes) => {
                got += bytes.len() as u64;
                if got > size {
                    let _ = std::fs::remove_file(&partial);
                    return Err(format!("{} came larger than the {size} bytes announced", at.display()));
                }
                if let Ok(open) = &mut file
                    && let Err(error) = open.write_all(&bytes)
                {
                    file = Err(error.to_string());
                }
            }
            Frame::End => break,
            other => {
                let _ = std::fs::remove_file(&partial);
                return Err(format!("expected a file's bytes, got {other:?}"));
            }
        }
    }
    let kept = file.and_then(|file| {
        if got != size {
            return Err(format!("{got} of its {size} bytes came; it changed while it was read"));
        }
        file.set_permissions(std::fs::Permissions::from_mode(mode & 0o777)).and_then(|()| std::fs::rename(&partial, at)).map_err(io)
    });
    if kept.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    Ok(kept)
}

// The worker takes each sent file into the run's folder under its name, before the command starts.
pub fn take(link: &mut Link, work: &Path, sends: &[SendAsk], halted: &dyn Fn() -> bool) -> Result<()> {
    for send in sends {
        receive(link, &work.join(&send.name), send.size, send.mode, halted)?.map_err(|error| format!("--send {}: {error}", send.name))?;
    }
    Ok(())
}

// The path in the run's folder, refused when any part of it is a symbolic link, which could lead out of the folder.
fn located(work: &Path, path: &Path) -> Result<PathBuf> {
    let mut at = work.to_path_buf();
    for part in path.components() {
        at.push(part);
        let meta = std::fs::symlink_metadata(&at).map_err(|_| format!("no {} in the run's folder to fetch", path.display()))?;
        if meta.file_type().is_symlink() {
            return Err(format!("{} is a symbolic link, and kebbie fetches none, as one can lead out of the run's folder", Path::new(at.strip_prefix(work).unwrap_or(&at)).display()));
        }
    }
    Ok(at)
}

// Every folder and plain file under at, folders before what is in them; a link or a special file is left out with a word.
fn walk(work: &Path, at: &Path, found: &mut Vec<(PathBuf, bool)>, left: &mut Vec<String>) {
    let shown = at.strip_prefix(work).unwrap_or(at).to_path_buf();
    let Ok(meta) = std::fs::symlink_metadata(at) else { return };
    if meta.is_dir() {
        found.push((shown, true));
        let mut names: Vec<PathBuf> = std::fs::read_dir(at).map(|entries| entries.filter_map(|entry| entry.ok().map(|entry| entry.path())).collect()).unwrap_or_default();
        names.sort();
        for name in names {
            walk(work, &name, found, left);
        }
    } else if meta.is_file() {
        found.push((shown, false));
    } else {
        left.push(format!("left out {}: kebbie fetches plain files and folders, no symbolic link or special file", shown.display()));
    }
}

fn path_text(path: &Path) -> Result<String> {
    path.to_str().map(str::to_string).ok_or_else(|| format!("left out {}: its name is no UTF-8", path.display()))
}

// The worker sends each fetched path back after the command, as an Entry and, for a file, its bytes, within what MAX_FILES leaves after the files sent; what it cannot send it says.
pub fn send_back(link: &mut Link, work: &Path, fetches: &[String], budget: u64, say: &dyn Fn(&mut Link, &str)) -> std::result::Result<(), std::io::Error> {
    let mut left = budget;
    let mut buffer = vec![0u8; CHUNK];
    for fetch in fetches {
        let found = inside(fetch).and_then(|path| located(work, &path));
        let at = match found {
            Ok(at) => at,
            Err(why) => {
                say(link, &why);
                continue;
            }
        };
        let (mut entries, mut notes) = (Vec::new(), Vec::new());
        walk(work, &at, &mut entries, &mut notes);
        for note in notes {
            say(link, &note);
        }
        for (path, dir) in entries {
            let text = match path_text(&path) {
                Ok(text) => text,
                Err(why) => {
                    say(link, &why);
                    continue;
                }
            };
            if dir {
                link.send(&Frame::Said(Said::Entry { path: text, dir: true, size: 0, mode: 0o755 }))?;
                continue;
            }
            let opened = OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW).open(work.join(&path)).and_then(|file| file.metadata().map(|meta| (file, meta)));
            let (file, meta) = match opened {
                Ok(opened) => opened,
                Err(error) => {
                    say(link, &format!("left out {text}: {error}"));
                    continue;
                }
            };
            if meta.len() > left {
                say(link, &format!("left out {text}: at {} it would take what a run sends and fetches in all past {LIMIT}", mib(meta.len())));
                continue;
            }
            left -= meta.len();
            link.send(&Frame::Said(Said::Entry { path: text, dir: false, size: meta.len(), mode: meta.permissions().mode() & 0o777 }))?;
            let mut file = file.take(meta.len());
            loop {
                let read = file.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                link.send(&Frame::Data(buffer[..read].to_vec()))?;
            }
            link.send(&Frame::End)?;
        }
    }
    Ok(())
}

// What came back so far, for the line kebbie run ends with.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Fetched {
    pub files: usize,
    pub bytes: u64,
    pub notes: Vec<String>,
}

impl Fetched {
    pub fn summary(&self, into: &Path) -> String {
        format!("[kebbie: fetched {} file{}, {}, into {}]\n", self.files, if self.files == 1 { "" } else { "s" }, mib(self.bytes), into.display())
    }
}

// The asker takes one Entry: only a path inside one it asked for, into the same path under into; a file it cannot write is drained and named.
pub fn take_entry(link: &mut Link, into: &Path, asked: &[String], budget: u64, entry: (String, bool, u64, u32), fetched: &mut Fetched) -> Result<()> {
    let (path, dir, size, mode) = entry;
    let relative = inside(&path).map_err(|_| format!("the worker sent back {path:?}, which is no path inside the run's folder"))?;
    if !asked.iter().filter_map(|asked| inside(asked).ok()).any(|asked| relative.starts_with(&asked)) {
        return Err(format!("the worker sent back {path}, which no --fetch asked for"));
    }
    let at = into.join(&relative);
    if dir {
        if let Err(error) = std::fs::create_dir_all(&at) {
            fetched.notes.push(format!("{}: {error}", at.display()));
        }
        return Ok(());
    }
    if fetched.bytes + size > budget {
        return Err(format!("the worker sent back more than the {LIMIT} a run sends and fetches in all"));
    }
    match receive(link, &at, size, mode, &|| false)? {
        Ok(()) => {
            fetched.files += 1;
            fetched.bytes += size;
        }
        Err(why) => fetched.notes.push(format!("{}: {why}", at.display())),
    }
    Ok(())
}

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::{Value, json};

use crate::sh::Result;

pub const VERSION: u64 = 1;
const POLL: Duration = Duration::from_millis(300);
const FRESH: u64 = 15;
const REMOTE: &str = "PATH=\"$HOME/.local/bin:$PATH\" kebbie job serve";
const REF: &str = "refs/kebbie/jobs";

// One request for a named job on another machine: the commit to run it on, and where the worker fetches that commit from.
#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    pub id: String,
    pub repo: String,
    pub name: String,
    pub sha: String,
    pub source: String,
    pub task: String,
    pub machine: String,
    pub at: u64,
}

impl Job {
    fn to_json(&self) -> Value {
        json!({ "v": VERSION, "id": self.id, "repo": self.repo, "name": self.name, "sha": self.sha, "source": self.source, "task": self.task, "machine": self.machine, "at": self.at })
    }

    fn from_json(value: &Value) -> Option<Job> {
        let text = |key: &str| value[key].as_str().filter(|text| !text.is_empty()).map(str::to_string);
        (value["v"] == VERSION).then_some(())?;
        Some(Job { id: text("id")?, repo: text("repo")?, name: text("name")?, sha: text("sha")?, source: text("source")?, task: text("task")?, machine: text("machine").unwrap_or_default(), at: value["at"].as_u64().unwrap_or(0) })
    }
}

fn dir(state: &Path) -> PathBuf {
    state.join("jobs")
}

fn part(state: &Path, id: &str, kind: &str) -> PathBuf {
    dir(state).join(format!("{id}.{kind}"))
}

fn workers(state: &Path) -> PathBuf {
    dir(state).join("workers")
}

// A repo, task or machine name becomes a folder or file name on another machine, so it keeps only what cannot leave its folder.
fn safe(name: &str) -> String {
    let kept: String = name.chars().map(|character| if character.is_ascii_alphanumeric() || "-_.@".contains(character) { character } else { '_' }).collect();
    if kept.is_empty() || kept.chars().all(|character| character == '.') { "_".to_string() } else { kept }
}

// The repo's name as both machines know it: the last part of its origin, else the main checkout's folder.
fn repo_name(top: &Path) -> String {
    let origin = crate::repo::git(top, &["config", "--get", "remote.origin.url"]).unwrap_or_default();
    let last = origin.trim_end_matches('/').trim_end_matches(".git").rsplit(['/', ':']).next().unwrap_or_default().to_string();
    if last.is_empty() { crate::repo::main_checkout(top).map_or_else(|_| crate::repo::name_of(top), |main| crate::repo::name_of(&main)) } else { last }
}

fn git_with_index(worktree: &Path, index: &Path, args: &[&str]) -> Result<String> {
    let output = crate::sh::git_command().current_dir(worktree).args(args).env("GIT_INDEX_FILE", index)
        .envs([("GIT_AUTHOR_NAME", "kebbie"), ("GIT_AUTHOR_EMAIL", "kebbie@localhost"), ("GIT_COMMITTER_NAME", "kebbie"), ("GIT_COMMITTER_EMAIL", "kebbie@localhost")])
        .output().map_err(|error| format!("git {}: {error}", args.join(" ")))?;
    if output.status.success() { Ok(String::from_utf8_lossy(&output.stdout).trim().to_string()) } else { Err(format!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&output.stderr).trim())) }
}

// The worktree as it stands, uncommitted and untracked files included, as a commit of its own: the branch, the index and the files are left as they are.
fn snapshot(worktree: &Path, id: &str) -> Result<String> {
    let head = crate::repo::git(worktree, &["rev-parse", "HEAD"])?;
    if crate::repo::git(worktree, &["status", "--porcelain"])?.is_empty() {
        return Ok(head);
    }
    let index = std::env::temp_dir().join(format!("kebbie-job-index-{id}"));
    let made = (|| {
        git_with_index(worktree, &index, &["read-tree", "HEAD"])?;
        git_with_index(worktree, &index, &["add", "--all"])?;
        let tree = git_with_index(worktree, &index, &["write-tree"])?;
        git_with_index(worktree, &index, &["commit-tree", &tree, "-p", &head, "-m", "The worktree as a kebbie job ran it"])
    })();
    let _ = std::fs::remove_file(&index);
    made
}

fn fresh_workers(state: &Path, now: u64) -> Vec<Value> {
    std::fs::read_dir(workers(state)).into_iter().flatten().flatten()
        .filter_map(|entry| std::fs::read_to_string(entry.path()).ok().and_then(|text| serde_json::from_str::<Value>(&text).ok()))
        .filter(|worker| worker["at"].as_u64().is_some_and(|at| now.saturating_sub(at) <= FRESH))
        .collect()
}

fn offers(offered: &Value, repo: &str, name: &str) -> bool {
    offered[repo].as_array().is_some_and(|names| names.iter().any(|offered| offered == name))
}

fn offered_by(state: &Path, repo: &str, name: &str, now: u64) -> Option<String> {
    fresh_workers(state, now).into_iter().find(|worker| offers(&worker["offers"], repo, name)).map(|worker| worker["machine"].as_str().unwrap_or("a worker").to_string())
}

fn no_worker(repo: &str, name: &str) -> String {
    format!("No worker offers the job {name} for {repo}. On the machine that runs it, list it under [repos.{repo}.jobs] in ~/.config/kebbie/worker.toml and start: kebbie worker <this machine's ssh name>")
}

// kebbie job run <name>: asks a connected worker to run its job of that name on this worktree as it stands, prints the job's output as it comes and ends as the job ended.
pub fn run(cwd: &Path, name: Option<&str>) -> Result<String> {
    let name = name.ok_or("kebbie job run needs the job's name, as the worker's worker.toml names it: kebbie job run build")?;
    let state = crate::lead::state_dir()?;
    let top = PathBuf::from(crate::repo::git(cwd, &["rev-parse", "--show-toplevel"])?);
    let repo = repo_name(&top);
    let worker = offered_by(&state, &repo, name, crate::messages::now()).ok_or_else(|| no_worker(&repo, name))?;
    let id = format!("{:x}-{}", crate::typing::now_ms(), std::process::id());
    let sha = snapshot(&top, &id)?;
    crate::repo::git(&top, &["update-ref", &format!("{REF}/{id}"), &sha])?;
    let task = crate::repo::git(&top, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_default();
    let job = Job { id, repo, name: name.to_string(), sha, source: top.to_string_lossy().to_string(), task: if task.is_empty() || task == "HEAD" { "detached".to_string() } else { task }, machine: crate::config::machine(), at: crate::messages::now() };
    crate::state::write(&part(&state, &job.id, "json"), job.to_json().to_string())?;
    println!("kebbie: {} runs {} for {} on {}", worker, job.name, job.repo, &job.sha[..12.min(job.sha.len())]);
    let ended = follow(&state, &job, &mut |text| print!("{text}"), &mut || std::thread::sleep(POLL), &mut crate::messages::now);
    forget(&state, &job, &top);
    ended
}

fn forget(state: &Path, job: &Job, top: &Path) {
    for kind in ["json", "claimed", "log", "result.json"] {
        let _ = std::fs::remove_file(part(state, &job.id, kind));
    }
    let _ = crate::repo::git(top, &["update-ref", "-d", &format!("{REF}/{}", job.id)]);
}

// Prints the job's log as it grows and ends with its result; a job no worker took, with no worker left that offers it, is given up.
fn follow(state: &Path, job: &Job, print: &mut dyn FnMut(&str), pause: &mut dyn FnMut(), now: &mut dyn FnMut() -> u64) -> Result<String> {
    let mut read = 0u64;
    let drain = |read: &mut u64, print: &mut dyn FnMut(&str)| {
        let Ok(mut file) = std::fs::File::open(part(state, &job.id, "log")) else { return };
        let mut text = String::new();
        if file.seek(SeekFrom::Start(*read)).is_ok() && file.read_to_string(&mut text).is_ok() && !text.is_empty() {
            *read += text.len() as u64;
            print(&text);
        }
    };
    let mut unoffered_since: Option<u64> = None;
    loop {
        drain(&mut read, print);
        if let Some(result) = std::fs::read_to_string(part(state, &job.id, "result.json")).ok().and_then(|text| serde_json::from_str::<Value>(&text).ok()) {
            drain(&mut read, print);
            let said = result["said"].as_str().unwrap_or_default();
            return match result["exit"].as_i64() {
                Some(0) => Ok(format!("kebbie: {} passed", job.name)),
                Some(code) => Err(format!("kebbie: {} failed with exit {code}{}", job.name, if said.is_empty() { String::new() } else { format!(": {said}") })),
                None => Err(format!("kebbie: {} ended without a result", job.name)),
            };
        }
        let at = now();
        if part(state, &job.id, "claimed").exists() || offered_by(state, &job.repo, &job.name, at).is_some() {
            unoffered_since = None;
        } else if at.saturating_sub(*unoffered_since.get_or_insert(at)) > FRESH {
            return Err(format!("The worker went away before it took the job. {}", no_worker(&job.repo, &job.name)));
        }
        pause();
    }
}

fn pending(state: &Path, offered: &Value) -> Option<Job> {
    let mut jobs: Vec<Job> = std::fs::read_dir(dir(state)).into_iter().flatten().flatten()
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".json") && !entry.file_name().to_string_lossy().ends_with(".result.json"))
        .filter_map(|entry| std::fs::read_to_string(entry.path()).ok().and_then(|text| serde_json::from_str::<Value>(&text).ok()).and_then(|value| Job::from_json(&value)))
        .filter(|job| offers(offered, &job.repo, &job.name) && !part(state, &job.id, "claimed").exists() && !part(state, &job.id, "result.json").exists())
        .collect();
    jobs.sort_by(|a, b| (a.at, &a.id).cmp(&(b.at, &b.id)));
    jobs.into_iter().find(|job| std::fs::OpenOptions::new().write(true).create_new(true).open(part(state, &job.id, "claimed")).is_ok())
}

fn end(state: &Path, id: &str, exit: i64, said: &str) {
    let _ = crate::state::write(&part(state, id, "result.json"), json!({ "exit": exit, "said": said }).to_string());
}

// One worker's side of the master, over the worker's own connection: it says what it offers, gets one job at a time, and its lines become the job's log and result.
pub fn serve() -> Result<String> {
    let state = crate::lead::state_dir()?;
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(std::result::Result::ok) {
            if tx.send(line).is_err() {
                return;
            }
        }
    });
    let mut out = std::io::stdout();
    serve_with(&state, &rx, &mut |line| writeln!(out, "{line}").and_then(|()| out.flush()).map_err(|error| error.to_string()), &mut crate::messages::now)
}

fn serve_with(state: &Path, lines: &mpsc::Receiver<String>, send: &mut dyn FnMut(&str) -> Result<()>, now: &mut dyn FnMut() -> u64) -> Result<String> {
    let hello: Value = lines.recv_timeout(Duration::from_secs(30)).ok().and_then(|line| serde_json::from_str(&line).ok()).ok_or("kebbie job serve is the master's end of kebbie worker: it reads the worker's hello first")?;
    if hello["v"] != VERSION {
        let refused = format!("this kebbie speaks jobs version {VERSION}, the worker sent {}; update the older kebbie", hello["v"]);
        let _ = send(&json!({ "refused": refused }).to_string());
        return Err(refused);
    }
    let machine = hello["machine"].as_str().unwrap_or("worker").to_string();
    let file = workers(state).join(format!("{}.json", safe(&machine)));
    let mut running: Option<String> = None;
    let ended = loop {
        crate::state::write(&file, json!({ "machine": machine, "offers": hello["offers"], "at": now(), "pid": std::process::id() }).to_string())?;
        match lines.recv_timeout(POLL) {
            Ok(line) => {
                let Ok(said) = serde_json::from_str::<Value>(&line) else { continue };
                let (Some(id), true) = (running.clone(), said["id"].as_str().is_some_and(|id| Some(id) == running.as_deref())) else { continue };
                if let Some(text) = said["log"].as_str() {
                    if let Ok(mut log) = crate::state::append_to(&part(state, &id, "log")) {
                        let _ = writeln!(log, "{text}");
                    }
                } else if let Some(exit) = said["exit"].as_i64() {
                    end(state, &id, exit, said["said"].as_str().unwrap_or_default());
                    running = None;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break Ok(String::new()),
        }
        if running.is_none()
            && let Some(job) = pending(state, &hello["offers"])
        {
            if let Err(error) = send(&job.to_json().to_string()) {
                end(state, &job.id, 255, "the worker's connection closed as the job was sent");
                break Err(error);
            }
            running = Some(job.id);
        }
    };
    if let Some(id) = running {
        end(state, &id, 255, "the worker disconnected before the job ended");
    }
    let _ = std::fs::remove_file(&file);
    ended
}

// What this machine runs for others: worker.toml's [repos.<repo>.jobs], each a name and the shell command it stands for.
#[derive(Debug, Default, PartialEq)]
pub struct Offered {
    repos: BTreeMap<String, BTreeMap<String, String>>,
}

impl Offered {
    pub fn parse(text: &str) -> Result<Offered> {
        let table: toml::Table = text.parse().map_err(|error| format!("worker.toml: {error}"))?;
        let mut repos = BTreeMap::new();
        for (repo, entry) in table.get("repos").and_then(toml::Value::as_table).into_iter().flatten() {
            let jobs: BTreeMap<String, String> = entry.get("jobs").and_then(toml::Value::as_table).into_iter().flatten()
                .filter_map(|(name, command)| command.as_str().filter(|command| !command.trim().is_empty()).map(|command| (name.clone(), command.to_string()))).collect();
            if !jobs.is_empty() {
                repos.insert(repo.clone(), jobs);
            }
        }
        if repos.is_empty() {
            return Err("worker.toml offers no job: give each repo its jobs, as [repos.app.jobs] with build = \"xcodebuild -scheme App build\"".to_string());
        }
        Ok(Offered { repos })
    }

    fn load() -> Result<Offered> {
        let file = crate::config::dir().join("worker.toml");
        let text = std::fs::read_to_string(&file).map_err(|_| format!("{} is missing: it names the jobs this machine runs for others, as [repos.app.jobs] with build = \"xcodebuild -scheme App build\"", file.display()))?;
        Offered::parse(&text)
    }

    fn hello(&self, machine: &str) -> Value {
        let offers: BTreeMap<&String, Vec<&String>> = self.repos.iter().map(|(repo, jobs)| (repo, jobs.keys().collect())).collect();
        json!({ "v": VERSION, "machine": machine, "offers": offers })
    }

    fn command(&self, repo: &str, name: &str) -> Option<&str> {
        self.repos.get(repo).and_then(|jobs| jobs.get(name)).map(String::as_str)
    }
}

fn data_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share/kebbie/worker")
}

// The job's commit in a folder of the task's own under this worker's data folder, fetched from the master over the connection the worker already has to it.
fn checkout(data: &Path, target: Option<&str>, job: &Job) -> Result<PathBuf> {
    let repo = data.join(safe(&job.repo));
    let store = repo.join(".store");
    if !store.join("HEAD").is_file() {
        std::fs::create_dir_all(&store).map_err(|error| format!("{}: {error}", store.display()))?;
        crate::repo::git(&store, &["init", "--quiet", "--bare"])?;
    }
    let from = target.map_or_else(|| job.source.clone(), |target| format!("{target}:{}", job.source));
    let fetched = format!("{REF}/{}", safe(&job.id));
    crate::repo::git(&store, &["fetch", "--quiet", "--force", &from, &format!("+{REF}/{}:{fetched}", job.id)])?;
    let work = repo.join(safe(&job.task));
    if work.join(".git").exists() {
        crate::repo::git(&work, &["checkout", "--quiet", "--detach", "--force", &job.sha])?;
        crate::repo::git(&work, &["clean", "-fdq"])?;
    } else {
        crate::repo::git(&store, &["worktree", "add", "--quiet", "--detach", &work.to_string_lossy(), &job.sha])?;
    }
    let _ = crate::repo::git(&store, &["update-ref", "-d", &fetched]);
    Ok(work)
}

// Runs one job the master sent: only a job this machine's worker.toml names for that repo, on the commit it fetched, its output sent back line by line.
fn run_job(offered: &Offered, data: &Path, target: Option<&str>, job: &Job, send: &mut dyn FnMut(Value) -> Result<()>) -> Result<()> {
    let done = |send: &mut dyn FnMut(Value) -> Result<()>, exit: i64, said: &str| send(json!({ "id": job.id, "exit": exit, "said": said }));
    let Some(command) = offered.command(&job.repo, &job.name) else {
        return done(send, 127, &format!("this worker's worker.toml names no job {} for {}", job.name, job.repo));
    };
    let work = match checkout(data, target, job) {
        Ok(work) => work,
        Err(error) => return done(send, 125, &format!("the worker could not get the commit: {error}")),
    };
    let child = Command::new("sh").args(["-c", &format!("{{ {command}\n}} 2>&1")]).current_dir(&work)
        .envs([("KEBBIE_JOB", job.name.as_str()), ("KEBBIE_JOB_TASK", job.task.as_str()), ("KEBBIE_JOB_SHA", job.sha.as_str()), ("KEBBIE_JOB_MASTER", job.machine.as_str())])
        .stdin(Stdio::null()).stdout(Stdio::piped()).spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(error) => return done(send, 126, &format!("the job's command did not start: {error}")),
    };
    if let Some(output) = child.stdout.take() {
        for line in BufReader::new(output).split(b'\n').map_while(std::result::Result::ok) {
            send(json!({ "id": job.id, "log": String::from_utf8_lossy(&line) }))?;
        }
    }
    let exit = child.wait().ok().and_then(|status| status.code()).map_or(255, i64::from);
    done(send, exit, "")
}

// One connection to the master, until it closes: Ok when the master ended it, Err with why it could not be made or was refused.
fn session(offered: &Offered, data: &Path, target: Option<&str>, machine: &str) -> Result<()> {
    let mut command = match target {
        Some(target) => {
            let mut ssh = Command::new("ssh");
            ssh.args(["-o", "BatchMode=yes", "-o", "ServerAliveInterval=15", "-o", "ServerAliveCountMax=3", target, REMOTE]);
            ssh
        }
        None => {
            let mut own = Command::new(crate::peers::own_exe()?);
            own.args(["job", "serve"]);
            own
        }
    };
    let mut child = command.stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().map_err(|error| format!("the connection to the master did not start: {error}"))?;
    let mut to_master = child.stdin.take().ok_or("no pipe to the master")?;
    let from_master = child.stdout.take().ok_or("no pipe from the master")?;
    let mut send = |value: Value| writeln!(to_master, "{value}").and_then(|()| to_master.flush()).map_err(|error| format!("the master's connection closed: {error}"));
    send(offered.hello(machine))?;
    let mut ended = Ok(());
    for line in BufReader::new(from_master).lines().map_while(std::result::Result::ok) {
        let Ok(said) = serde_json::from_str::<Value>(&line) else { continue };
        if let Some(refused) = said["refused"].as_str() {
            ended = Err(format!("the master refused this worker: {refused}"));
            break;
        }
        let Some(job) = Job::from_json(&said) else { continue };
        eprintln!("kebbie worker: {} for {} ({}) on {}", job.name, job.repo, job.task, &job.sha[..12.min(job.sha.len())]);
        if let Err(error) = run_job(offered, data, target, &job, &mut send) {
            ended = Err(error);
            break;
        }
    }
    drop(send);
    drop(to_master);
    let _ = child.kill();
    let _ = child.wait();
    ended
}

// kebbie worker <ssh name>: this machine dials the master and runs the jobs its own worker.toml names, so nothing on it listens; --local serves a master on this same machine, --once ends with the first connection.
pub fn worker(target: Option<&str>, local: bool, once: bool) -> Result<String> {
    let offered = Offered::load()?;
    let target = match (target, local) {
        (_, true) => None,
        (Some(target), false) => Some(target),
        (None, false) => return Err("kebbie worker needs the master's ssh name, as you would ssh to it: kebbie worker devbox (--local serves a master on this machine)".to_string()),
    };
    let machine = crate::config::machine();
    let data = data_dir();
    loop {
        let ended = session(&offered, &data, target, &machine);
        if once {
            return ended.map(|()| String::new());
        }
        eprintln!("kebbie worker: {}; connecting again in 5 s", ended.err().unwrap_or_else(|| "the master's connection ended".to_string()));
        std::thread::sleep(Duration::from_secs(5));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(id: &str, repo: &str, name: &str, at: u64) -> Job {
        Job { id: id.to_string(), repo: repo.to_string(), name: name.to_string(), sha: "a".repeat(40), source: "/src/app".to_string(), task: "billing".to_string(), machine: "devbox".to_string(), at }
    }

    #[test]
    fn a_worker_offers_only_the_jobs_its_own_file_names_and_a_file_with_none_is_refused() {
        let offered = Offered::parse("[repos.app.jobs]\nbuild = \"xcodebuild -scheme App build\"\ntest = \"xcodebuild test\"\n\n[repos.empty.jobs]\nblank = \"  \"\n").unwrap();
        assert_eq!(offered.command("app", "build"), Some("xcodebuild -scheme App build"));
        assert_eq!((offered.command("app", "deploy"), offered.command("other", "build"), offered.command("empty", "blank")), (None, None, None), "a name the file does not list is never a command");
        assert_eq!(offered.hello("mac"), json!({ "v": 1, "machine": "mac", "offers": { "app": ["build", "test"] } }), "the master hears names, never the commands");
        assert!(Offered::parse("[repos.app]\npath = \"x\"\n").unwrap_err().contains("offers no job") && Offered::parse("not toml =").is_err());
    }

    #[test]
    fn names_from_the_other_machine_never_leave_their_folder() {
        assert_eq!((safe("../../etc"), safe(".."), safe(""), safe("fix/login page"), safe("billing@app")), (".._.._etc".to_string(), "_".to_string(), "_".to_string(), "fix_login_page".to_string(), "billing@app".to_string()), "one folder name, whatever was sent");
    }

    #[test]
    fn the_master_hands_a_worker_the_oldest_job_it_offers_once_and_takes_its_log_and_result() {
        let state = crate::sh::test_dir("jobs-serve");
        for waiting in [job("b", "app", "build", 20), job("a", "app", "build", 10), job("c", "other", "build", 5), job("d", "app", "deploy", 1)] {
            crate::state::write(&part(&state, &waiting.id, "json"), waiting.to_json().to_string()).unwrap();
        }
        let (tx, rx) = mpsc::channel();
        tx.send(json!({ "v": 1, "machine": "mac", "offers": { "app": ["build"] } }).to_string()).unwrap();
        let mut sent: Vec<Value> = Vec::new();
        let mut clock = 100;
        let mut worker = Some(tx);
        serve_with(&state, &rx, &mut |line| {
            let job: Value = serde_json::from_str(line).unwrap();
            let id = job["id"].as_str().unwrap().to_string();
            let answer = worker.as_ref().unwrap();
            answer.send(json!({ "id": id, "log": format!("building {id}") }).to_string()).unwrap();
            answer.send(json!({ "id": "someone-else", "exit": 0 }).to_string()).unwrap();
            answer.send(json!({ "id": id, "exit": if id == "a" { 0 } else { 65 }, "said": "" }).to_string()).unwrap();
            sent.push(job);
            if sent.len() == 2 {
                worker.take();
            }
            Ok(())
        }, &mut || { clock += 1; if clock > 140 { panic!("the master never ran dry") } clock }).unwrap();
        assert_eq!(sent.iter().map(|job| job["id"].as_str().unwrap()).collect::<Vec<_>>(), ["a", "b"], "oldest first, one at a time, and never a repo or a job the worker did not offer");
        assert_eq!(std::fs::read_to_string(part(&state, "a", "log")).unwrap(), "building a\n");
        let result = |id: &str| serde_json::from_str::<Value>(&std::fs::read_to_string(part(&state, id, "result.json")).unwrap()).unwrap()["exit"].clone();
        assert_eq!((result("a"), result("b")), (json!(0), json!(65)), "a result for another job's id ends nothing");
        assert!(!part(&state, "c", "claimed").exists() && !part(&state, "d", "claimed").exists());
        let _ = std::fs::remove_dir_all(state);
    }

    #[test]
    fn a_worker_that_disconnects_mid_job_ends_the_job_as_failed_and_leaves_no_worker_listed() {
        let state = crate::sh::test_dir("jobs-disconnect");
        crate::state::write(&part(&state, "a", "json"), job("a", "app", "build", 10).to_json().to_string()).unwrap();
        let (tx, rx) = mpsc::channel();
        tx.send(json!({ "v": 1, "machine": "mac", "offers": { "app": ["build"] } }).to_string()).unwrap();
        let mut hang_up = Some(tx);
        serve_with(&state, &rx, &mut |_| { hang_up.take(); Ok(()) }, &mut || 100).unwrap();
        let result: Value = serde_json::from_str(&std::fs::read_to_string(part(&state, "a", "result.json")).unwrap()).unwrap();
        assert_eq!((result["exit"].clone(), result["said"].as_str()), (json!(255), Some("the worker disconnected before the job ended")));
        assert!(fresh_workers(&state, 100).is_empty(), "a worker that is gone offers nothing");
        let (old, rx) = mpsc::channel();
        old.send(json!({ "v": 0, "machine": "mac", "offers": {} }).to_string()).unwrap();
        let mut refusal = String::new();
        assert!(serve_with(&state, &rx, &mut |line| { refusal = line.to_string(); Ok(()) }, &mut || 100).is_err() && refusal.contains("refused"), "another version of the format is refused in words: {refusal}");
        let _ = std::fs::remove_dir_all(state);
    }

    #[test]
    fn a_job_prints_its_log_as_it_grows_ends_as_the_job_ended_and_gives_up_when_no_worker_is_left() {
        let state = crate::sh::test_dir("jobs-follow");
        let waiting = job("a", "app", "build", 10);
        std::fs::create_dir_all(workers(&state)).unwrap();
        crate::state::write(&workers(&state).join("mac.json"), json!({ "machine": "mac", "offers": { "app": ["build"] }, "at": 100 }).to_string()).unwrap();
        assert_eq!(offered_by(&state, "app", "build", 105).as_deref(), Some("mac"));
        assert_eq!((offered_by(&state, "app", "test", 105), offered_by(&state, "app", "build", 200)), (None, None), "a job it does not offer, and a worker not heard from");
        let mut printed = String::new();
        let mut step = 0;
        let passed = follow(&state, &waiting, &mut |text| printed.push_str(text), &mut || {
            step += 1;
            match step {
                1 => std::fs::write(part(&state, "a", "log"), "compiling\n").unwrap(),
                2 => std::fs::write(part(&state, "a", "log"), "compiling\nlinking\n").unwrap(),
                _ => end(&state, "a", 0, ""),
            }
        }, &mut || 105);
        assert_eq!((passed, printed.as_str()), (Ok("kebbie: build passed".to_string()), "compiling\nlinking\n"), "each line once");
        end(&state, "a", 65, "");
        assert_eq!(follow(&state, &waiting, &mut |_| {}, &mut || {}, &mut || 105), Err("kebbie: build failed with exit 65".to_string()), "a failed job fails the command, so the task's Claude and its test record see it");
        std::fs::remove_file(part(&state, "a", "result.json")).unwrap();
        let mut clock = 200;
        let gone = follow(&state, &waiting, &mut |_| {}, &mut || {}, &mut || { clock += 5; clock }).unwrap_err();
        assert!(gone.starts_with("The worker went away before it took the job.") && gone.contains("kebbie worker"), "{gone}");
        let _ = std::fs::remove_dir_all(state);
    }

    #[test]
    fn a_worker_runs_the_masters_worktree_as_it_stands_and_sends_back_every_line_and_the_exit() {
        let root = crate::sh::test_dir("jobs-run");
        let (master, data) = (root.join("master/app"), root.join("worker-data"));
        std::fs::create_dir_all(&master).unwrap();
        let git = |args: &[&str]| crate::repo::git(&master, args).unwrap();
        git(&["init", "--quiet", "-b", "main"]);
        std::fs::write(master.join("tracked.txt"), "committed\n").unwrap();
        git(&["add", "."]);
        git(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "--quiet", "-m", "one"]);
        git(&["checkout", "--quiet", "-b", "billing"]);
        let head = git(&["rev-parse", "HEAD"]);
        assert_eq!(snapshot(&master, "clean").unwrap(), head, "a clean worktree runs as its commit");
        std::fs::write(master.join("tracked.txt"), "edited\n").unwrap();
        std::fs::write(master.join("new.txt"), "untracked\n").unwrap();
        let sha = snapshot(&master, "dirty").unwrap();
        assert!(sha != head && git(&["rev-parse", "HEAD"]) == head && git(&["status", "--porcelain"]).lines().count() == 2, "the branch, the index and the files are left as they were");
        git(&["update-ref", &format!("{REF}/j1"), &sha]);
        let offered = Offered::parse("[repos.app.jobs]\nbuild = \"cat tracked.txt new.txt; echo to-stderr >&2; echo $KEBBIE_JOB_TASK; exit 3\"\n").unwrap();
        let run = |job: &Job| {
            let mut said: Vec<Value> = Vec::new();
            run_job(&offered, &data, None, job, &mut |value| { said.push(value); Ok(()) }).unwrap();
            said
        };
        let first = Job { id: "j1".to_string(), repo: "app".to_string(), name: "build".to_string(), sha: sha.clone(), source: master.to_string_lossy().to_string(), task: "billing".to_string(), machine: "devbox".to_string(), at: 1 };
        let said = run(&first);
        assert_eq!(said.iter().filter_map(|line| line["log"].as_str()).collect::<Vec<_>>(), ["edited", "untracked", "to-stderr", "billing"], "uncommitted and untracked files run too, and stderr is in the log");
        assert_eq!(said.last().unwrap()["exit"], json!(3), "the job's own exit comes back");
        std::fs::write(data.join("app/billing/left-over.txt"), "from the last run").unwrap();
        std::fs::remove_file(master.join("new.txt")).unwrap();
        let again = snapshot(&master, "again").unwrap();
        git(&["update-ref", &format!("{REF}/j2"), &again]);
        let said = run(&Job { id: "j2".to_string(), sha: again, ..first.clone() });
        assert_eq!(said.iter().filter_map(|line| line["log"].as_str()).next_back(), Some("billing"));
        assert!(!data.join("app/billing/new.txt").exists() && !data.join("app/billing/left-over.txt").exists(), "the task's folder is reused and holds the new commit alone");
        let unlisted = run(&Job { name: "deploy".to_string(), ..first.clone() });
        assert_eq!((unlisted.len(), unlisted[0]["exit"].clone()), (1, json!(127)), "a job the worker's file does not name runs nothing: {unlisted:?}");
        let unreachable = run(&Job { id: "j9".to_string(), ..first });
        assert_eq!(unreachable[0]["exit"], json!(125), "a commit it cannot fetch is said, not run");
        let _ = std::fs::remove_dir_all(root);
    }
}

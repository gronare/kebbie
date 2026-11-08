use std::path::{Component, Path, PathBuf};

use crate::push_guard::{Simple, base, simple_commands};

const PROGRAMS: [&str; 2] = ["kubectl", "helm"];
const SETTERS: [&str; 7] = ["export", "declare", "typeset", "readonly", "local", "unset", "set"];
// Flags that pick another kubeconfig, cluster, login or identity, or make kubectl or helm write or run something on this machine.
const ESCAPES: [&str; 20] = [
    "--kubeconfig", "--context", "--cluster", "--user", "--token", "--server", "--as", "--as-group", "--as-uid", "--certificate-authority", "--username", "--password",
    "--cache-dir", "--profile-output", "--log-file", "--log-dir", "--output-directory", "--helm-command", "--post-renderer", "--post-renderer-args",
];
const ESCAPE_PREFIXES: [&str; 3] = ["--client-", "--kube-", "--enable-"];
const KUBECTL: [&str; 44] = [
    "annotate", "api-resources", "api-versions", "apply", "attach", "auth", "autoscale", "certificate", "cluster-info", "completion", "config", "cordon", "create", "debug", "delete", "describe",
    "diff", "drain", "edit", "events", "exec", "explain", "expose", "get", "help", "kustomize", "label", "logs", "patch", "port-forward", "proxy", "replace", "rollout", "run", "scale", "set",
    "taint", "top", "uncordon", "version", "wait", "alpha", "options", "cp",
];
const HELM: [&str; 31] = [
    "completion", "create", "dependency", "dep", "env", "get", "help", "history", "hist", "install", "lint", "list", "ls", "package", "pull", "fetch", "push", "registry", "repo", "rollback",
    "search", "show", "inspect", "status", "template", "test", "uninstall", "delete", "un", "upgrade", "version",
];
const READ_CONFIG: [&str; 5] = ["view", "current-context", "get-contexts", "get-clusters", "get-users"];
const VALUED: [&str; 12] = ["-n", "--namespace", "-v", "--v", "--request-timeout", "--tls-server-name", "--profile", "--registry-config", "--repository-cache", "--repository-config", "--burst-limit", "--qps"];
const LOADED: [&str; 3] = [".claude", ".git", ".mcp.json"];

fn off() -> String {
    format!(
        "This task's profile names no kubeconfig, so kebbie refuses kubectl and helm here: they would run with your own cluster logins. To let a profile's tasks reach a cluster, set kubeconfig under [sandbox] in its file in {} to a kubeconfig for a limited ServiceAccount; kebbie's how-to \"Manage a cluster from a scheduled task\" walks through it.",
        crate::profiles::dir().display()
    )
}

fn leaves(what: &str) -> String {
    format!("{what} would leave the kubeconfig this task's profile names: kubectl and helm run here with that file alone, KUBECONFIG already points at it, and what they may do is what its login's RBAC allows. Drop it, or ask the user if the task needs another cluster or login.")
}

// The folders a task's kubectl and helm may name files in: the worktree the command runs in and the task's temp folder.
pub fn within(cwd: Option<&Path>) -> Vec<PathBuf> {
    let worktree = cwd.and_then(|cwd| cwd.ancestors().find(|dir| dir.join(".git").exists()).or(Some(cwd))).map(Path::to_path_buf);
    worktree.into_iter().chain(std::env::var_os("TMPDIR").filter(|tmp| !tmp.is_empty()).map(PathBuf::from)).collect()
}

fn names_kubeconfig(word: &str) -> bool {
    word.split_once('=').map_or(word, |(name, _)| name) == crate::sandbox::KUBECONFIG
}

// A file outside the worktree and temp folder, or one Claude Code or git runs as the user, which kubectl or helm would read or write outside the sandbox.
fn outside(value: &str, within: &[PathBuf]) -> bool {
    let path = Path::new(value);
    if value == "/dev/null" {
        return false;
    }
    if value.starts_with('~') {
        return true;
    }
    let inside = if path.is_absolute() { within.iter().find_map(|root| path.strip_prefix(root).ok()) } else { Some(path) };
    let Some(inside) = inside else { return true };
    inside.components().any(|part| part == Component::ParentDir) || inside.components().next().is_some_and(|first| LOADED.iter().any(|loaded| first.as_os_str() == *loaded))
}

fn path_like(value: &str) -> bool {
    value.starts_with(['/', '~', '.']) || value.contains("/..")
}

// A word and each value after an = in it, as --from-file=key=/path or --set-file key=/path name a file.
fn files_named<'a>(word: &'a str, within: &[PathBuf]) -> Option<&'a str> {
    std::iter::once(word).chain(word.match_indices('=').map(|(at, _)| &word[at + 1..])).find(|value| path_like(value) && outside(value, within))
}

fn escapes(name: &str, command: &Simple, within: &[PathBuf]) -> Option<String> {
    let own_environment = |word: &String| crate::push_guard::assignment(word) || word.starts_with('-') || ["env", "sudo", "doas"].contains(&base(word));
    if command.before.iter().any(own_environment) {
        return Some(leaves(&format!("Running {name} after {}", command.before.join(" "))));
    }
    if let Some(file) = command.redirects.iter().find_map(|target| files_named(target, within)) {
        return Some(leaves(&format!("{name} redirected to {file}, a file outside this task's worktree,")));
    }
    let args = &command.words[1..];
    let mut positionals = Vec::new();
    let mut raw = false;
    let mut words = args.iter().take_while(|word| *word != "--").peekable();
    while let Some(word) = words.next() {
        let (flag, value) = word.split_once('=').map_or((word.as_str(), None), |(flag, value)| (flag, Some(value)));
        if flag.starts_with("--") {
            if ESCAPES.contains(&flag) || ESCAPE_PREFIXES.iter().any(|prefix| flag.starts_with(prefix)) {
                return Some(leaves(&format!("{name} {flag}")));
            }
            raw = raw || flag == "--raw";
            if flag == "--raw" && value.is_none() {
                words.next();
                continue;
            }
            if flag != "--raw" && value.and_then(|value| files_named(value, within)).is_some() {
                return Some(leaves(&format!("{name} {word}, a file outside this task's worktree,")));
            }
            if value.is_none() && VALUED.contains(&flag) {
                words.next();
            }
        } else if flag.starts_with('-') && flag.len() > 1 {
            if name == "kubectl" && flag.starts_with("-s") {
                return Some(leaves(&format!("{name} {flag}")));
            }
            if VALUED.contains(&flag) {
                words.next();
            }
        } else if let Some(file) = files_named(word, within) {
            return Some(leaves(&format!("{name} naming {file}, a file outside this task's worktree,")));
        } else {
            positionals.push(word.as_str());
        }
    }
    let known: &[&str] = if name == "kubectl" { &KUBECTL } else { &HELM };
    match positionals.first().copied() {
        Some(sub) if !known.contains(&sub) => Some(format!("{name} {sub} is not one of {name}'s own commands, so it would run a {name} plugin from this machine outside the sandbox; kebbie refuses it in a task.")),
        Some("cp") => Some(format!("{name} cp copies files between this machine and a pod outside the sandbox; kebbie refuses it in a task. Use {name} exec with the file's content, or ask the user.")),
        Some("config") if !positionals.get(1).is_some_and(|read| READ_CONFIG.contains(read)) || raw => Some(leaves(&format!("{name} {}", positionals.iter().take(2).copied().collect::<Vec<_>>().join(" ")))),
        _ => None,
    }
}

// What a task's kubectl and helm may reach: nothing, the profile's kubeconfig alone, or, with the sandbox off and no kubeconfig, whatever the user's own login reaches.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Cluster {
    None,
    Kubeconfig,
    Own,
}

impl Cluster {
    pub const KUBE_FLAG: &str = "--kube";
    pub const OWN_FLAG: &str = "--own-cluster";

    pub fn flagged(kube: bool, own: bool) -> Cluster {
        if kube { Cluster::Kubeconfig } else if own { Cluster::Own } else { Cluster::None }
    }

    // A kubeconfig holds kubectl and helm to it; without one an unsandboxed task's kubectl is the user's own, which the guard cannot hold anyway.
    pub fn of(policy: &crate::sandbox::Policy, sandboxed: bool) -> Cluster {
        if policy.kubeconfig.is_some() { Cluster::Kubeconfig } else if sandboxed { Cluster::None } else { Cluster::Own }
    }

    pub fn flag(self) -> Option<&'static str> {
        match self {
            Cluster::Kubeconfig => Some(Self::KUBE_FLAG),
            Cluster::Own => Some(Self::OWN_FLAG),
            Cluster::None => None,
        }
    }
}

// Why kebbie refuses a shell line in a task: kubectl or helm in a sandboxed task whose profile names no kubeconfig, or a way past the one it names.
pub fn refusal(line: &str, cluster: Cluster, within: &[PathBuf]) -> Option<String> {
    if cluster == Cluster::Own {
        return None;
    }
    let kube = cluster == Cluster::Kubeconfig;
    for command in simple_commands(line) {
        let program = command.words.first().map(|first| base(first));
        let sets = match program {
            None => command.before.iter().any(|word| names_kubeconfig(word)),
            Some(setter) if SETTERS.contains(&setter) => command.words[1..].iter().any(|word| names_kubeconfig(word)),
            Some(_) => false,
        };
        if sets && kube {
            return Some(leaves("Setting KUBECONFIG"));
        }
        let Some(name) = program.filter(|name| PROGRAMS.contains(name)) else { continue };
        if !kube {
            return Some(off());
        }
        if let Some(why) = escapes(name, &command, within) {
            return Some(why);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORKTREE: &str = "/src/wt1.infra";

    fn refused(line: &str) -> Option<String> {
        refusal(line, Cluster::Kubeconfig, &[PathBuf::from(WORKTREE), PathBuf::from("/tmp/kebbie-501/wt1.infra")])
    }

    #[test]
    fn a_sandboxed_task_whose_profile_names_no_kubeconfig_runs_no_kubectl_or_helm_and_is_told_how_to_enable_it() {
        for line in ["kubectl get pods", "helm list -A", "/opt/homebrew/bin/kubectl version", "env kubectl get nodes", "sh -c 'kubectl get ns'", "git status && helm repo update"] {
            let why = refusal(line, Cluster::None, &[]).unwrap_or_else(|| panic!("{line:?} gets through"));
            assert!(why.contains("names no kubeconfig") && why.contains("set kubeconfig under [sandbox]") && why.contains("Manage a cluster from a scheduled task"), "{why}");
        }
        for line in ["git status", "cargo test kubectl", "grep -r kubectl docs", "echo helm", "rg 'kubectl get' src"] {
            assert_eq!(refusal(line, Cluster::None, &[]), None, "{line:?}");
        }
    }

    #[test]
    fn an_unsandboxed_task_without_a_kubeconfig_runs_kubectl_and_helm_with_the_users_own_login() {
        let policy = crate::sandbox::Policy::default();
        assert_eq!((Cluster::of(&policy, true), Cluster::of(&policy, false)), (Cluster::None, Cluster::Own));
        let kube = crate::sandbox::Policy { kubeconfig: Some("~/.kube/kebbie-ops.yaml".to_string()), ..Default::default() };
        assert_eq!((Cluster::of(&kube, true), Cluster::of(&kube, false)), (Cluster::Kubeconfig, Cluster::Kubeconfig), "a kubeconfig holds with the sandbox off too");
        for line in ["kubectl get pods", "kubectl --context admin get ns", "helm --kube-context prod list", "KUBECONFIG=~/.kube/other kubectl get ns"] {
            assert_eq!(refusal(line, Cluster::Own, &[]), None, "{line:?}");
        }
        assert_eq!(Cluster::flagged(false, true), Cluster::Own);
        assert_eq!(Cluster::flagged(false, false), Cluster::None, "no flag refuses");
    }

    #[test]
    fn a_redirect_is_this_machines_shell_so_kubectl_writes_no_file_outside_the_worktree_even_after_a_double_dash() {
        for line in ["kubectl exec web-0 -- cat /etc/app.conf > ~/file", "kubectl exec web-0 -- cat x >> /home/ada/.zshrc", "kubectl logs web-0 &> ~/.profile", "kubectl get cm x -o yaml >.claude/settings.json", "kubectl apply -f - < /home/ada/.ssh/id_ed25519", "kubectl exec -i web-0 -- sh < ../secret.sh"] {
            assert!(refused(line).is_some_and(|why| why.contains("redirected to")), "{line:?}: {:?}", refused(line));
        }
        for line in ["kubectl exec web-0 -- ls /", "kubectl exec web-0 -- cat /etc/hosts > out/hosts.txt", "kubectl get pods 2>&1 | grep Crash", "kubectl get pods > /tmp/kebbie-501/wt1.infra/pods.txt", "kubectl logs web-0 2>/dev/null"] {
            assert_eq!(refused(line), None, "{line:?}");
        }
    }

    #[test]
    fn kubectl_and_helm_keep_to_the_profiles_kubeconfig_in_every_form_a_shell_reaches_them_by() {
        for line in [
            "kubectl --kubeconfig ~/.kube/config get pods", "kubectl --kubeconfig=/home/ada/.kube/config get pods", "kubectl --context admin get pods", "kubectl get pods --context=prod",
            "kubectl --cluster other get ns", "kubectl --user admin get ns", "kubectl --token abc get ns", "kubectl --server https://10.0.0.1:6443 get ns", "kubectl -s https://x get ns",
            "kubectl -shttps://x get ns", "kubectl --as system:admin get secrets", "kubectl --as-group system:masters get secrets", "kubectl --certificate-authority /etc/ca.crt get ns",
            "kubectl --client-certificate a.crt --client-key a.key get ns", "kubectl --username u --password p get ns", "helm --kubeconfig ~/.kube/config list", "helm --kube-context admin list",
            "helm --kube-token abc list", "helm --kube-apiserver https://x list", "helm --kube-as-user admin list", "KUBECONFIG=~/.kube/config kubectl get pods", "env KUBECONFIG=/x kubectl get pods",
            "env -u KUBECONFIG kubectl get pods", "env -i kubectl get pods", "sudo kubectl get pods", "PATH=/tmp/x:$PATH kubectl evil", "export KUBECONFIG=~/.kube/config; kubectl get pods",
            "unset KUBECONFIG && kubectl get pods", "KUBECONFIG=~/.kube/config", "sh -c 'kubectl --context admin get ns'", "bash -lc \"helm --kube-context admin list\"", "eval kubectl --context admin get ns",
            "echo \"$(kubectl --context admin get ns)\"", "nohup kubectl --context x get ns &", "timeout 30 kubectl --context x get ns", "xargs kubectl --context x get < names",
            "kubectl config use-context admin", "kubectl config set-credentials x --token y", "kubectl config view --raw", "kubectl config", "kubectl cp ./a web-0:/tmp/a", "kubectl cp web-0:/etc/passwd /tmp/p",
            "kubectl evil-plugin", "helm plugin install https://x", "helm diff upgrade web ./chart", "helm install web ./chart --post-renderer ./render.sh",
            "kubectl apply -f /home/ada/.ssh/id_ed25519", "kubectl create secret generic x --from-file=/home/ada/.aws/credentials", "kubectl create secret generic x --from-file ~/.ssh/id_ed25519",
            "kubectl apply -f ../../other/secret.yaml", "kubectl get pods > ~/.zshrc", "kubectl get cm x -o yaml > .claude/settings.json", "helm template web ./chart --output-dir /home/ada/.local/bin",
            "helm install web ./chart --set-file key=/home/ada/.ssh/id_ed25519", "kubectl kustomize --enable-helm --helm-command ./x .", "kubectl apply -k . --enable-alpha-plugins",
            "kubectl get pods --cache-dir /home/ada/.config", "kubectl cluster-info dump --output-directory /home/ada", "kubectl apply -f /src/wt1.infra/../secret.yaml",
        ] {
            assert!(refused(line).is_some(), "{line:?} gets through");
            let decision: serde_json::Value = serde_json::from_str(&crate::push_guard::hook(&serde_json::json!({ "tool_name": "Bash", "tool_input": { "command": line }, "cwd": WORKTREE }).to_string(), Cluster::Kubeconfig)).unwrap();
            assert_eq!(decision["hookSpecificOutput"]["permissionDecision"], "deny", "{line:?}");
        }
        assert!(refused("kubectl --context admin get pods").unwrap().contains("kubectl --context would leave the kubeconfig this task's profile names") && refused("kubectl --context admin get pods").unwrap().contains("RBAC"));
    }

    #[test]
    fn a_task_with_a_kubeconfig_manages_its_cluster_with_kubectl_and_helm_as_usual() {
        for line in [
            "kubectl get pods -n web", "kubectl -n web get deploy/web -o jsonpath='{.status.readyReplicas}'", "kubectl --namespace=web rollout status deploy/web", "kubectl get --raw /readyz",
            "kubectl apply -f deploy/web.yaml", "kubectl apply -f /src/wt1.infra/deploy/web.yaml", "kubectl apply -f /tmp/kebbie-501/wt1.infra/patch.yaml", "kubectl apply -k overlays/prod",
            "kubectl diff -f manifests/", "kubectl logs deploy/web --since=1h", "kubectl exec web-0 -- /bin/sh -c 'ls /etc'", "kubectl run probe --image=busybox --rm -i -- wget -qO- http://web",
            "kubectl config current-context", "kubectl config view --minify", "kubectl auth can-i list pods", "kubectl top nodes", "kubectl get events -A --sort-by=.lastTimestamp",
            "kubectl get pods | grep Crash", "cd charts && helm lint web", "helm upgrade --install web ./charts/web -n web -f values/prod.yaml --atomic --wait", "helm list -A", "helm repo add stable https://charts.example.org",
            "helm history web -n web", "helm rollback web 3 -n web", "helm template web ./charts/web -f values.yaml", "kubectl version --client", "echo $KUBECONFIG", "kubectl get nodes 2>/dev/null",
        ] {
            assert_eq!(refused(line), None, "{line:?}");
        }
    }

    #[test]
    fn the_task_settings_give_the_guard_the_profiles_kubeconfig_and_codexs_hook_reads_it_from_the_tasks_profile() {
        let mut kube = crate::profiles::built_in(crate::profiles::TASK).unwrap();
        kube.sandbox.kubeconfig = Some("/home/ada/.kube/kebbie-ops.yaml".to_string());
        let guards = |profile: &crate::profiles::Profile, sandboxed: bool| -> Vec<String> {
            let written: serde_json::Value = serde_json::from_str(&crate::agent::Agent::Claude.task_settings("/k", "billing@app", None, None, profile, sandboxed)).unwrap();
            written["hooks"]["PreToolUse"].as_array().unwrap().iter().filter(|entry| entry["matcher"] == "Bash").map(|entry| entry["hooks"][0]["command"].as_str().unwrap().to_string()).collect()
        };
        let task = crate::profiles::built_in(crate::profiles::TASK).unwrap();
        for sandboxed in [true, false] {
            assert_eq!(guards(&kube, sandboxed), ["'/k' hook push-guard --kube"]);
        }
        assert_eq!(guards(&task, true), ["'/k' hook push-guard"], "a sandboxed task without one refuses kubectl");
        assert_eq!(guards(&task, false), ["'/k' hook push-guard --own-cluster"], "an unsandboxed one runs it with the user's login");
        let main = include_str!("main.rs");
        assert!(main.contains("push_guard::hook(&input, kube_guard::Cluster::flagged(arguments.flag(\"--kube\"), arguments.flag(\"--own-cluster\")))"), "the flags reach the guard");
        let codex = include_str!("agent/codex.rs");
        assert!(codex.contains("crate::push_guard::hook(input, cwd.as_deref().map_or(crate::kube_guard::Cluster::None, crate::tasks::cluster_at))"), "Codex's hook asks the task's own profile");
        let launch = include_str!("tasks/launch.rs");
        assert!(launch.contains("Ok((profile, None)) => crate::kube_guard::Cluster::of(&profile.sandbox, profile.sandbox.on(configured))"), "and its sandbox, as the start decides it");
    }
}

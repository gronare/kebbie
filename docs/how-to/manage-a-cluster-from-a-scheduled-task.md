# Manage a cluster from a scheduled task

Give tasks a login to a Kubernetes cluster that does only what you allow, and let a routine check or fix the cluster on a schedule.

A task reaches no cluster by default. A profile that names a kubeconfig file runs its tasks' `kubectl` and `helm` outside the sandbox with that file alone. What the task may do in the cluster is what that login's RBAC allows, so you choose the level when you make the login.

This page sets up three levels, each its own login, kubeconfig and profile:

| Level | Profile | What its login may do |
| --- | --- | --- |
| read | `cluster-read` | Read everything except Secrets and RBAC. |
| operate | `cluster-operate` | Read, restart workloads and delete pods. |
| full | `cluster-full` | Anything, deletes included (`cluster-admin`). |

## Make a login for each level

Run these from your own terminal, with your own admin access. A task's shell refuses `kubectl config` writes and `--kubeconfig`.

1. Save the ServiceAccounts and their RBAC as `kebbie-rbac.yaml`:

   ```yaml
   apiVersion: v1
   kind: Namespace
   metadata:
     name: kebbie
   ---
   apiVersion: v1
   kind: ServiceAccount
   metadata: { name: kebbie-read, namespace: kebbie }
   ---
   apiVersion: v1
   kind: ServiceAccount
   metadata: { name: kebbie-operate, namespace: kebbie }
   ---
   apiVersion: v1
   kind: ServiceAccount
   metadata: { name: kebbie-full, namespace: kebbie }
   ---
   # read: the built-in view role, which leaves out Secrets and RBAC, plus the cluster-scoped objects it does not cover
   apiVersion: rbac.authorization.k8s.io/v1
   kind: ClusterRole
   metadata: { name: kebbie-cluster-reads }
   rules:
     - apiGroups: [""]
       resources: [nodes, namespaces, persistentvolumes]
       verbs: [get, list, watch]
     - apiGroups: [storage.k8s.io]
       resources: [storageclasses, volumeattachments]
       verbs: [get, list, watch]
   ---
   # operate: restart a workload (kubectl rollout restart patches it) and delete a pod
   apiVersion: rbac.authorization.k8s.io/v1
   kind: ClusterRole
   metadata: { name: kebbie-operate }
   rules:
     - apiGroups: [apps]
       resources: [deployments, statefulsets, daemonsets]
       verbs: [patch]
     - apiGroups: [""]
       resources: [pods]
       verbs: [delete]
   ---
   apiVersion: rbac.authorization.k8s.io/v1
   kind: ClusterRoleBinding
   metadata: { name: kebbie-read-view }
   roleRef: { apiGroup: rbac.authorization.k8s.io, kind: ClusterRole, name: view }
   subjects: [{ kind: ServiceAccount, name: kebbie-read, namespace: kebbie }, { kind: ServiceAccount, name: kebbie-operate, namespace: kebbie }]
   ---
   apiVersion: rbac.authorization.k8s.io/v1
   kind: ClusterRoleBinding
   metadata: { name: kebbie-read-cluster-reads }
   roleRef: { apiGroup: rbac.authorization.k8s.io, kind: ClusterRole, name: kebbie-cluster-reads }
   subjects: [{ kind: ServiceAccount, name: kebbie-read, namespace: kebbie }, { kind: ServiceAccount, name: kebbie-operate, namespace: kebbie }]
   ---
   apiVersion: rbac.authorization.k8s.io/v1
   kind: ClusterRoleBinding
   metadata: { name: kebbie-operate }
   roleRef: { apiGroup: rbac.authorization.k8s.io, kind: ClusterRole, name: kebbie-operate }
   subjects: [{ kind: ServiceAccount, name: kebbie-operate, namespace: kebbie }]
   ---
   apiVersion: rbac.authorization.k8s.io/v1
   kind: ClusterRoleBinding
   metadata: { name: kebbie-full }
   roleRef: { apiGroup: rbac.authorization.k8s.io, kind: ClusterRole, name: cluster-admin }
   subjects: [{ kind: ServiceAccount, name: kebbie-full, namespace: kebbie }]
   ---
   # a token per account that does not expire, so a routine keeps working
   apiVersion: v1
   kind: Secret
   metadata: { name: kebbie-read-token, namespace: kebbie, annotations: { kubernetes.io/service-account.name: kebbie-read } }
   type: kubernetes.io/service-account-token
   ---
   apiVersion: v1
   kind: Secret
   metadata: { name: kebbie-operate-token, namespace: kebbie, annotations: { kubernetes.io/service-account.name: kebbie-operate } }
   type: kubernetes.io/service-account-token
   ---
   apiVersion: v1
   kind: Secret
   metadata: { name: kebbie-full-token, namespace: kebbie, annotations: { kubernetes.io/service-account.name: kebbie-full } }
   type: kubernetes.io/service-account-token
   ```

   To keep a level to some namespaces, bind its role with a RoleBinding in each namespace instead of a ClusterRoleBinding.

2. Apply it:

   ```bash
   kubectl apply -f kebbie-rbac.yaml
   ```

3. Write a kubeconfig for the read level, readable by you alone:

   ```bash
   LEVEL=read
   FILE=~/.kube/kebbie-cluster-$LEVEL.yaml
   SERVER=$(kubectl config view --minify -o jsonpath='{.clusters[0].cluster.server}')
   kubectl get secret kebbie-$LEVEL-token -n kebbie -o jsonpath='{.data.ca\.crt}' | base64 -d > /tmp/kebbie-ca.crt
   (umask 077
    kubectl --kubeconfig "$FILE" config set-cluster cluster --server "$SERVER" --certificate-authority /tmp/kebbie-ca.crt --embed-certs
    kubectl --kubeconfig "$FILE" config set-credentials kebbie-$LEVEL --token "$(kubectl get secret kebbie-$LEVEL-token -n kebbie -o jsonpath='{.data.token}' | base64 -d)"
    kubectl --kubeconfig "$FILE" config set-context kebbie-$LEVEL --cluster cluster --user kebbie-$LEVEL
    kubectl --kubeconfig "$FILE" config use-context kebbie-$LEVEL)
   chmod 600 "$FILE"
   rm /tmp/kebbie-ca.crt
   ```

4. Run step 3 again with `LEVEL=operate`, then with `LEVEL=full`.

5. Check what a login may do:

   ```bash
   kubectl --kubeconfig ~/.kube/kebbie-cluster-read.yaml auth can-i --list
   ```

## Point a profile at each login

1. Write one profile per level in `~/.config/kebbie/profiles/`:

   ```toml
   # ~/.config/kebbie/profiles/cluster-read.toml
   for = "reading the cluster's state: health checks, reports"

   [sandbox]
   kubeconfig = "~/.kube/kebbie-cluster-read.yaml"
   ```

   ```toml
   # ~/.config/kebbie/profiles/cluster-operate.toml
   for = "restarting workloads and clearing stuck pods"

   [sandbox]
   kubeconfig = "~/.kube/kebbie-cluster-operate.yaml"
   ```

   ```toml
   # ~/.config/kebbie/profiles/cluster-full.toml
   for = "cluster changes you ask for by name, deletes included"

   [sandbox]
   kubeconfig = "~/.kube/kebbie-cluster-full.yaml"
   ```

2. Check them:

   ```bash
   kebbie doctor
   ```

   Each profile has a `kubeconfig` line reading `is there and yours alone (mode 0600)`.

## Check and fix the cluster on a schedule

1. Add a routine on the read login for the check:

   ```bash
   kebbie routine add --title "Cluster health" --repo infra --every 6h --profile cluster-read \
     "Check the cluster with kubectl: every node Ready, no pod crash-looping or pending for long, every volume attached."
   ```

2. Add one on the operate login for the fix:

   ```bash
   kebbie routine add --title "Restart stuck pods" --repo infra --at 06:00 --mode fix --profile cluster-operate \
     "Find pods stuck in CrashLoopBackOff for over an hour. Delete each one once, or restart its workload with kubectl rollout restart, and report what you did and what is still failing."
   ```

   If you keep routines in a repo's own config instead, such as `~/.config/kebbie/repos/infra.toml`, the same two as tables:

   ```toml
   [[routines]]
   id = "cluster-health"
   title = "Cluster health"
   every = "6h"
   profile = "cluster-read"
   prompt = "Check the cluster with kubectl: every node Ready, no pod crash-looping or pending for long, every volume attached."

   [[routines]]
   id = "restart-stuck-pods"
   title = "Restart stuck pods"
   at = "06:00"
   mode = "fix"
   profile = "cluster-operate"
   prompt = "Find pods stuck in CrashLoopBackOff for over an hour. Delete each one once, or restart its workload with kubectl rollout restart, and report what you did and what is still failing."
   ```

3. Start a run now to see it work:

   ```bash
   kebbie routine run cluster-health
   ```

The task's start says `kubectl and helm run outside the sandbox in this task with ~/.kube/kebbie-cluster-read.yaml alone`. When the cluster is healthy, the task closes itself and you hear nothing; when it finds something, it waits under Needs you with its findings.

Keep `cluster-full` for tasks you start yourself, with `kebbie start --profile cluster-full`, not for a routine.

## What holds a task to its login

- Its `kubectl` and `helm` get `KUBECONFIG` set to the profile's file. The sandboxed shell, and Claude's Read and Edit tools, still read none of `~/.kube` nor the file itself.
- kebbie refuses a `kubectl` or `helm` command that picks another login or cluster (`--kubeconfig`, `--context`, `--cluster`, `--user`, `--token`, `--server`, `--as`, `--client-*`, helm's `--kube-*`), sets or unsets `KUBECONFIG`, writes `kubectl config`, runs a plugin or `--post-renderer`, uses `kubectl cp`, or names a file outside the worktree and the task's temp folder. A shell redirect (`>`, `<`) is checked the same way, also after `--`, since this machine's shell runs it.
- In a sandboxed task whose profile names no kubeconfig, kebbie refuses `kubectl` and `helm` outright and says how to enable them. With the sandbox off they run with your own logins, and the task's start says so.

> **Warning:** These refusals are best effort: they read the command line. The real limit is the login's own RBAC, so give each level only what its tasks need.

- If the profile runs Codex (`agent = "codex"`), its commands get `KUBECONFIG` and the same refusals, but Codex runs `kubectl` outside its sandbox only once its approval is given. Under `permission_mode = "auto"` Codex's reviewer answers; otherwise the scheduled task waits for you.
- If `kebbie doctor` warns that a profile's kubeconfig is your default `~/.kube/config`, that file is usually your own admin login: point the profile at a file of its own.
- If the file is readable by others, `kebbie doctor` names the `chmod 600` that fixes it.
- If you link a profile file in from a team repo, it may not set `kubeconfig`: the login is picked on this machine only.

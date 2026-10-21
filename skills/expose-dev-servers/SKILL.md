---
name: expose-dev-servers
description: Serve each worktree's dev server through Caddy at its own host, so it opens in the laptop's browser while kelpie runs on a remote Linux machine reached over ssh. Use when the user runs /kelpie:expose-dev-servers or asks to open the dev servers of a remote machine or devbox from their laptop.
---

# Expose dev servers to the laptop

`kelpie` is `${KELPIE_BIN:-$HOME/.local/share/kelpie/current/target/release/kelpie}`. The main checkout is the parent of `git rev-parse --path-format=absolute --git-common-dir`. The routes file is `/etc/caddy/kelpie.caddy`.

`kelpie routes` prints a Caddy site for every pooled worktree with a port, in each repo whose repo config has `worktree.dev_route = true`: the host its `worktree.dev_url` names, proxied to `localhost:<port>`. `--plain-http` serves each as `http://<host>` for a proxy in front that ends TLS. Routing is **opt-in per repo**: a repo is routed only on the user's yes in step 2.

Ask one short question at a time and wait for the answer. Every write outside the repo configs (a package, a file under `/etc`) waits for the user's yes.

1. Confirm the situation. `SSH_CONNECTION` is set and `uname -s` says `Linux`; otherwise say this skill serves dev servers from a remote Linux machine and stop. Then ask, one at a time: the wildcard domain whose names point at this machine (`*.dev.example.com`, `*.devbox`), and who ends TLS: a proxy in front that holds a wildcard cert and forwards plain HTTP to this machine's port 80, Caddy here, or nothing (plain http). Done when you have the domain and the TLS answer.
2. Pick the repos. The repo is the one the user names, else the current main checkout; ask whether others should join. For each, read its own word on worktrees: README.md, AGENTS.md, CLAUDE.md, CONTRIBUTING.md, `bin/setup-worktree` and each `.claude/skills/*/SKILL.md` about worktrees, in the main checkout. A repo that already serves each worktree at its own host (per-worktree domains, its own proxy, puma-dev, a tunnel such as cloudflared, ngrok or `tailscale serve`) stays out: say what it uses and leave it. Done when each repo is either left out with its reason or a candidate.
3. Propose each candidate's keys, in its repo config:

   ```toml
   [worktree]
   dev_route = true
   dev_url = "https://{repo}-{number}.dev.example.com"
   ```

   With `http://` when nothing ends TLS. `{repo}-{number}` keeps the host one label under the domain, which a one-level wildcard cert covers (`{worktree}` holds a dot). When `kelpie config --repo <main>` shows a `dev_url` already, show both and ask which wins, since the board links to it. Write only on the user's yes, as `/kelpie:configure` describes. Done when `kelpie config --repo <main>` shows `worktree.dev_route = true  (repo config)` and the `dev_url` agreed.
4. Install Caddy when `command -v caddy` finds none: ask first, since it is a system package. Read https://caddyserver.com/docs/install for the distro `/etc/os-release` names (the official apt repo on Debian and Ubuntu, the COPR repo with `dnf` on Fedora and RHEL) and run those commands. Done when `caddy version` prints and `systemctl is-active caddy` says `active`.
5. Write the routes, after asking to write under `/etc`:
   - `kelpie routes` (with `--plain-http` when a proxy in front ends TLS) `| sudo tee /etc/caddy/kelpie.caddy >/dev/null`.
   - `/etc/caddy/Caddyfile` holds the line `import kelpie.caddy`; add it once, keeping the rest.
   - When Caddy ends TLS for names the internet cannot reach, the Caddyfile's global options block (the first block, `{ … }`) holds `local_certs`. The laptop then trusts Caddy's root, `/var/lib/caddy/.local/share/caddy/pki/authorities/local/root.crt`: tell the user to copy it to the laptop and add it to the keychain or trust store.
   - `caddy validate --config /etc/caddy/Caddyfile`, then `sudo systemctl reload caddy`.

   Done when validate says `Valid configuration`, the reload exits 0, and you have read the routes file's last line: each skipped worktree there has a reason the user accepts (a worktree without a port gets one from `/kelpie:set-up-worktree`).
6. Tell the user what lives outside this machine, since this skill changes no other machine: a wildcard record (`*.dev.example.com` → this machine's LAN, WireGuard or Tailscale address), or the front proxy's route for those hosts to `http://<this machine>:80` with the `Host` header kept; and the firewall here letting the laptop reach port 80 (443 when Caddy ends TLS). Done when the user says it is in place, or `getent hosts <a routed host>` from the laptop's side resolves to this machine.
7. Make the dev servers accept the host. Caddy runs here, so a server bound to localhost is reached; what refuses is a host allow-list: Rails `config.hosts`, Vite `server.allowedHosts`, Django `ALLOWED_HOSTS`, webpack-dev-server `allowedHosts`. Find where the repo sets it from its own docs, propose the domain there as one change, and commit it only on the user's yes. Done when each candidate repo accepts `*.<domain>` or the user declined.
8. Verify one worktree. Start its dev server (`w` on the board, or `PORT=<port> <worktree.dev_server>` in the worktree), then on this machine `curl -sI -H "Host: <host>" http://localhost` (with Caddy ending TLS: `curl -skI --resolve <host>:443:127.0.0.1 https://<host>`). Stop the server if you started it. Done when the answer is the app's (no 502, no host refusal), and the user has opened the board's link for that task on the laptop and seen the app.
9. Reply in two lines: the hosts now routed and the repos left out with why; and that a new worktree joins by a **refresh**: the command the routes file's first line names (`kelpie routes` or `kelpie routes --plain-http`) `| sudo tee /etc/caddy/kelpie.caddy >/dev/null`, then the validate and the reload of step 5. `/kelpie:set-up-worktree` refreshes when it gives a routed repo's worktree its port.

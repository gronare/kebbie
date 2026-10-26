# Open dev servers from a remote machine

Run kebbie on a VM or devbox you reach over ssh, and open every worktree's dev server in your laptop's browser at its own address, such as `https://app-1.dev.example.com`.

Caddy on the machine serves one site per worktree and proxies it to that worktree's port. Routing is opt-in per repo: a repo that already serves its worktrees itself, through its own proxy, puma-dev or a tunnel, leaves `worktree.dev_route` off and kebbie never routes it.

> **Tip:** The fastest way is to ask Claude on the machine to run `/kebbie:expose-dev-servers`. It asks for your domain and who ends TLS, turns the repo on on your yes, installs Caddy, writes the routes and checks one worktree end to end. The steps below are what it does.

## What you need

- The laptop reaches the machine directly: on the LAN, over WireGuard or over Tailscale.
- A wildcard name that points at the machine, such as `*.dev.example.com` or `*.devbox`, in your DNS or in the proxy in front.
- Each worktree has a port: `PORT=` in its `.env` or a port in its setup note. The setup skill picks one.

## 1. Turn the repo on

In the repo config, `~/.config/kebbie/repos/app.toml`:

```toml
[worktree]
dev_route = true
dev_url = "https://{repo}-{number}.dev.example.com"
```

- `dev_route` is read from the repo config only, never from config.toml.
- `dev_url` is also the board's link, so the task's `web` line opens the same address.
- `{repo}-{number}` keeps each host one label under the domain, which a one-level wildcard certificate covers. `{worktree}` (`wt1.app`) would add a label.
- Use `http://` when nothing ends TLS.

## 2. Print the routes

```bash
kebbie routes
```

```
# kebbie routes: a site per pooled worktree with a port in the repos with worktree.dev_route; rerun it when the pool changes

https://app-1.dev.example.com {
	reverse_proxy localhost:3001
}

https://app-2.dev.example.com {
	reverse_proxy localhost:3002
}

# 2 sites; skipped 1: no port: wt3.app; 1 repo without worktree.dev_route left out
```

kebbie looks at every repo it knows on the machine: those with a Space open in herdr, and the checkouts with a worktree pool under `KEBBIE_REPO_DIRS`. Each repo uses its own `dev_url`.

These get no site, and the last line says which and why:

- a worktree without a port,
- a `dev_url` on localhost or without a host,
- a host another worktree already has.

If a proxy in front holds the certificate and forwards plain HTTP to the machine's port 80, print each site as `http://app-1.dev.example.com` instead:

```bash
kebbie routes --plain-http
```

## 3. Load them into Caddy

1. Write the routes to Caddy's folder:

   ```bash
   kebbie routes | sudo tee /etc/caddy/kebbie.caddy >/dev/null
   ```

2. Add one line to `/etc/caddy/Caddyfile`:

   ```
   import kebbie.caddy
   ```

3. Check the config:

   ```bash
   caddy validate --config /etc/caddy/Caddyfile
   ```

4. Reload Caddy:

   ```bash
   sudo systemctl reload caddy
   ```

- If Caddy ends TLS for names the internet cannot reach, put `local_certs` in the Caddyfile's global options block and trust Caddy's root on the laptop: `/var/lib/caddy/.local/share/caddy/pki/authorities/local/root.crt`.

## 4. Point the names at the machine

- **DNS**: a wildcard record `*.dev.example.com` to the machine's LAN, WireGuard or Tailscale address. Open port 80, and 443 when Caddy ends TLS, to the laptop.
- **A proxy in front**: route `*.dev.example.com` to `http://<machine>:80` and keep the `Host` header.

## 5. Let the dev server accept the host

Caddy passes the browser's `Host` on. A dev server bound to localhost is fine, since Caddy runs on the same machine; what refuses is a host allow-list.

Add the domain to Rails `config.hosts`, Vite `server.allowedHosts`, Django `ALLOWED_HOSTS` or your framework's equivalent.

## 6. Check one

1. Start a worktree's server with `w` on the board.
2. On the machine, ask it through Caddy:

   ```bash
   curl -sI -H "Host: app-1.dev.example.com" http://localhost
   ```

   The app's own answer means the route works.

3. From the laptop, open the task's link on the board.

## When the pool grows

A new worktree gets a site when you print the routes again and reload. The setup skill does it for you when it gives a worktree of a routed repo its port and `/etc/caddy/kebbie.caddy` exists.

See also: [Run a task's dev server](run-dev-servers.md), [`worktree.dev_route`](../reference/configuration.md#keys).

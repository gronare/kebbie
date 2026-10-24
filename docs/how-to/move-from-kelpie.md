# Move from kelpie to kebbie

kebbie was called kelpie. An install made under the old name moves over in four steps; your tasks, backlog, settings and hub pairing come along.

herdr keeps a plugin under the id in its manifest and refuses an update that changes the id, so `kelpie update` cannot reach kebbie. The first step is the one kebbie cannot do for you.

1. Stop the old keeper, install kebbie in herdr and remove kelpie there:

   ```sh
   kelpie keeper --stop
   herdr plugin install --ref "$(curl -fsSL https://api.github.com/repos/gronare/kebbie/releases/latest | jq -r .tag_name)" gronare/kebbie
   herdr plugin uninstall kelpie
   ```

   On the main channel, install with `--ref main` instead.

2. Restart herdr. kebbie's startup links `~/.local/share/kebbie/current`, and kebbie's first run moves its folders:

   | Was | Is |
   | --- | --- |
   | `~/.local/state/kelpie` (or under `$XDG_STATE_HOME`) | `~/.local/state/kebbie` |
   | `~/.config/kelpie` (or under `$XDG_CONFIG_HOME`) | `~/.config/kebbie` |

   A folder moves only when the new one does not exist yet. When both exist, kebbie uses the new one and says so once. With `KEBBIE_STATE_DIR` (or the old `KELPIE_STATE_DIR`) set, the state folder stays where it points.

3. Run setup from the new install:

   ```sh
   ~/.local/share/kebbie/current/target/release/kebbie setup
   ```

   Before its usual steps it lists what is left of kelpie and asks before fixing each: herdr's kelpie plugin if it is still there, Claude Code's `kelpie@kelpie` plugin and `kelpie` marketplace, Codex's copy of kelpie's plugin, the `~/.local/bin/kelpie` link, `~/.local/share/kelpie`, and the word kelpie in herdr's `config.toml` (keys such as `kelpie.lead`, the sidebar sync's path), shown line by line before it is written. Then it links `~/.local/bin/kebbie`, installs `kebbie@kebbie` and reloads herdr's config. `--yes` fixes them all without asking, except a herdr config that is a link. `kebbie doctor` and `kebbie update` name whatever is still left.

   With a profile that runs Codex, run `kebbie setup codex` too; Codex asks you to trust kebbie's hooks again, as they are another plugin's now.

4. Move the hub. Once your hub answers at its new address, point this machine at it; the machine keeps its keys and its account:

   ```sh
   kebbie hub move https://hub.kebbie.dev
   ```

   A phone keeps its keys per address, so on the new one it is a new device: sign in there, add it with `kebbie hub add-device` (or `H` on the board), and remove the old entry with `kebbie hub remove-device`.

## What still reads the old names

- Environment variables: each `KELPIE_` variable you set is read as its `KEBBIE_` name while the new one is unset, and kebbie says once which ones to rename. `kebbie doctor` keeps listing them.
- Your notify command: it gets `KELPIE_EVENT`, `KELPIE_TASK`, `KELPIE_REPO` and `KELPIE_TEXT` alongside the `KEBBIE_` ones, so a command written for kelpie keeps working. Rename them in `notify` in `config.toml` when you can.
- Claude Code's `statusLine`: if it still runs kelpie's path, `kebbie doctor` shows the line to put in `~/.claude/settings.json`.

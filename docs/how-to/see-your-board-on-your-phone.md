# See your board on your phone

Follow your tasks from your phone through a [kebbie hub](https://github.com/gronare/kebbie-hub): every task grouped as on the board, a task's steps under its row, and the questions waiting for you with their options.

It works on any phone or browser you sign in on with your passkey. Every device reads, the backlog included; one where you allow changes also answers questions, chooses on an agent's prompt, pushes, adds ideas and starts backlog items.

![H connects this machine to a hub: the card with the code to check on the page and the link to open](../assets/clips/hub-connect.webp)

## What the hub sees

Every board the machine sends is end-to-end encrypted to your account's key and signed by the machine. Only your passkeys unlock that key, so the hub relays the board without being able to read it.

The hub cannot act on the machine by itself: the machine takes an answer, a prompt choice, a push, an idea or a start only from a device your account's key signed and the machine approved, and only the kinds [`hub.commands`](../reference/configuration.md#keys) allows. The board carries task names, repo names, summaries and questions, never a local path, a host name or a dev server address.

## What you need

- A browser where you can sign in to the hub with a passkey, or make an account there.
- The keeper running on the machine (`kebbie doctor` says so); it posts the board. With `[keeper] enabled = false`, an open board posts it once a minute instead.

## Connect the machine

1. On the board, press `H`. Or, in your own terminal, run:

   ```bash
   kebbie hub connect
   ```

2. Choose the hub:

   - `k`: the kebbie hub, at `hub.kebbie.dev`, which is free to connect and read.
   - `o`: your own, a server you run kebbie-hub on. Give its address. kebbie checks that it answers as a kebbie hub over `https` (`http` only for a hub on this machine), says `✓ answers as a kebbie hub`, then saves it as [`[hub] url`](../reference/configuration.md#keys).

   In a terminal `↑` `↓` move the highlight and Enter chooses, `k` and `o` choose at once and `esc` cancels. Without a terminal kebbie asks for `k` or `o`, with `k` the answer to Enter. With `[hub] url` set, kebbie connects there without asking.

3. Name the machine, once. The phone shows it by its one name, [`[machine] name`](../reference/configuration.md#keys), the same name your other machines on the vault see it by.

   Set already, kebbie uses it; `--name <name>` sets it; otherwise kebbie asks, and Enter keeps the host's short name. kebbie writes the name to config.toml once the hub took it.

4. Open the link. kebbie shows a card with a QR code of the link, the machine and the hub, a code such as `Z78K-XF53` and how long the code has left. Nothing opens by itself:

   - press Enter to open the link in your browser (not over SSH), and the card's `⏎ open in browser · or scan with your phone` becomes `✓ Opened in your browser`;
   - or scan the QR code with your phone.

5. On the page, check that it shows the same code.
6. Sign in with your passkey, or make an account.
7. Press Connect.

kebbie waits up to 10 minutes and ends on a card saying the machine is connected to the hub and that you sign in with your passkey on any phone or browser to see its board. Or it ends in one line: cancelled in the browser, or the code expired. `kebbie hub status` shows the account's id and the fingerprints of the two keys the machine pinned.

- If the QR code does not fit, make the pane larger: in terminals other than Ghostty, kitty and WezTerm it is drawn twice as wide and tall. `KEBBIE_QR=octant` draws it small when your terminal draws octant blocks well.
- If kebbie runs without a terminal, it prints the whole link and opens nothing.
- If this machine has no browser, `c` copies the link to paste elsewhere. The link shown under the card stops at its `#`, since what follows is the secret that ties your browser to this machine.
- If config.toml is a link, say into a dotfiles repo shared by several machines, kebbie never writes it: it says which line to set there or in a config.toml of this machine's own.

### How the QR code is drawn

It is drawn in the terminal's own colours, the text colour making the light modules and the background the dark ones, so it reads dark on light in a dark theme. In Ghostty, kitty and WezTerm it is in octant blocks; in other terminals it is drawn in half blocks, which every font draws.

### How the key reaches the machine

When you press Connect, the browser hands the machine your account's key, with no digits to compare. The link carries a secret that never reaches the hub, and the machine pins the key only when the browser's commitment to it matches that secret. From then on the machine seals every board to your account.

The machine's keys and the hub's token for it stay in `hub/` in kebbie's state directory, readable by you alone. Nothing prints them.

> **Warning:** A machine approved by typing its code on the page, without the link, holds no account key, so no browser can read its board and kebbie posts none. kebbie says so on the card, in the board's `H` window, in `kebbie hub status` and in `kebbie doctor`. Connect it from the link: `kebbie hub leave`, then `kebbie hub connect`, and open the link it shows.

### Rename the machine

Change `[machine] name`, or press `n` in the board's `H` window. The keeper renames it on the hub with its next post, and `kebbie hub status` does it at once. Your phone shows the new name when it next reads the board.

- If the hub is unreachable, `kebbie hub status` and `kebbie doctor` show the name the hub still has and why the rename failed.
- If `[machine] name` is unset, the hub keeps the name the machine connected with, even when the host is renamed.

## See the board on your phone

1. Open the hub in the phone's browser, `hub.kebbie.dev` or your own.
2. Sign in with your passkey.

The phone shows the board of every machine your account connected. Nothing is added on the machine: any phone or browser you sign in on reads the boards.

Above the groups, Next lists what needs you in the order `N` on the board and `kebbie next` give it, one list per machine, with how long each has waited and a line counting what is working, queued and parked; with nothing waiting it says Nothing needs you. Each task shows its title, its agent with the model its profile names, its recipe and the review assessment, and a line says when a machine is in away mode. Below the groups, Queued and Parked list the tasks set aside, a parked one with whether a question waits on it, and Routines list each routine's next run and last outcome.

A prompt on a task's agent's screen, such as Codex asking to run a command, shows under Questions and counts in its badge: the question, the command apart from it, and each option with the key it takes. With changes allowed on the device, you choose a plain option there; an option that takes your own words is answered at the machine.

The open task shows its agent, profile and recipe, its steps, its rules check as on the board, and a review section: the assessment, the reviewer's verdict, whether the push waits for it, and a second opinion's points, which never hold the push.

A task that works in [steps](work-in-steps.md) lists them under its row as on the board: the pushed ones as a count, then each step with its number, title, state (planned, in progress, ready or accepted) and its commits. With [`task.require_accept`](work-in-steps.md#require-every-step-accepted) on, the row says how many steps wait for your accept and which one is next, and the top of the board counts them across tasks as `◆ N to accept`; without it, the phone says accepting each step is optional and push takes what is committed. You accept a step on the machine, with `kebbie accept` or in the review split, or in its review on the phone.

The board kebbie sends needs a hub whose web app reads it: an older one refuses the board, so update the hub first, then kebbie. An older kebbie keeps working with a newer hub, without Next and the rest.

Reading needs a passkey that can unlock the account's key, such as one in iCloud Keychain, Google Password Manager or 1Password. A passkey that cannot signs in, and the page says it can't unlock the boards.

To keep the hub on the phone's home screen:

- iPhone: in Safari, tap Share, then **Add to Home Screen**.
- Android: in Chrome, open the menu and tap **Add to Home screen** (or **Install app**).

## Answer and push from your phone

Each device starts view only. Allow changes on the ones you use to answer and push:

1. Open **Account** on the device.
2. Under **This device**, turn on **Allow changes from this device**. It says `Changes go to each machine that takes them.`
3. Open a task that asks a question or is ready to push, and answer or push from it.

The task shows the answer as sent, then the machine's own reply once it applied it. A device left view only shows `View only on this device. Allow changes in Account` on a task instead of the buttons.

What reaches a machine is its own choice, set by [`hub.commands`](../reference/configuration.md#keys) in its config.toml. **Machines** says it under each one: `takes answers and pushes`, `takes answers, not pushes` or `takes nothing from the hub`.

- If a machine takes answers but not pushes, its task says `Not pushed: devbox takes no pushes from the hub. Push at the machine, or allow pushes in its kebbie settings.`
- A task in [steps](work-in-steps.md) pushes through one of its ready steps: the steps up to it go out whole, through that step's last commit, as `p` in the board's push window takes them, and the later ones stay in the task. If that step's last commit changed since your phone showed it, the push is refused; a step on the default branch already says there is nothing to push.
- If you turn the switch off, the device sends nothing; the setting stays in that browser alone.
- If the hub has not enabled answering for your account, the task says so instead.

### Work the backlog from your phone

**Backlog** lists each machine's backlog as its board's Backlog column shows it, with whether an item is to write, drafting or ready and how many of its questions wait on you. Open an item to read it in full. A device that makes changes also:

- **Adds an idea.** Tap **New idea**, write it and pick the machine. It is filed as `kebbie backlog add` files it, and the lead writes it out, hearing that it came from your phone.
- **Answers an open question.** Pick the question on the item and write your answer. It waits with the lead until the lead folds it into the item, on the phone and on the board's card alike.
- **Starts the item.** The machine starts it as `g` on the board does: the item is the task's prompt, and the lead picks how the task goes.
- **Edits the item.** Tap **Edit** on its title or on a section, change it and save. The machine writes it into the item's file or vault note.

- If questions still wait on you, the start is refused and says how many; start it again to start it anyway.
- The phone hears `Starting <task>` while the machine starts it. If the start fails, the device that asked gets a `Not started` notification that says why, and the item stays in the backlog.
- If the question changed on the item since you read it, the answer is refused; read the item again.
- If the item is already running as a task, or left the backlog, the machine says so.
- A note in a vault two machines share is the same item on both; start it from one.
- If the section changed on the machine since you opened it, the edit is refused; read it again and edit what it says now. A required section cannot be saved empty.
- An edit to a vault note needs a vault command that states the `write` capability, see [The vault contract](../reference/vault-contract.md#capabilities).

### Review a step from your phone

A task whose step waits for your acceptance offers a review on its row, and any ready or accepted step opens from its steps. The review shows the step's commits, each with its message and its own diff, and every file the whole step changes, in the order the review split shows them. A device that makes changes also:

- **Comments on a line.** Write a comment under any line, then **Send**. Every waiting comment is typed into the task's Claude as one message, each line shaped `path:line — text` as the review split sends them.
- **Marks a file read.** The read box in a file's header sets the same read mark the review split shows, in the whole step or in one commit, until the file changes.
- **Accepts the step.** **Accept** at the end accepts every commit of the step as `kebbie accept --step` does, and the board shows it accepted.

- A binary file, or one too large to send, shows as left out; review it at the machine.
- If the task's branch moved since you opened it, a read mark or an accept is refused; open the review again.
- If its Claude has text in its input box, the comments are not sent; send them again once it is clear.

### Approve a new device on the machine

Each machine decides which of your devices may change things on it, by [`hub.trust`](../reference/configuration.md#machine). A new device's first command waits on the machine until you let the device in.

1. Answer or push from the new device. The task shows that it waits for the machine.
2. On the machine, press `N` on the board. The device stands first, under `APPROVE OR DENY`.
3. Press `⏎` to see what it sent and its key. Check that the key matches the one under **Account** on the device.
4. Press `a` to approve. Or, in your own terminal, run:

   ```bash
   kebbie hub devices approve "iPhone 16"
   ```

What it sent goes in at once, and the device makes changes on this machine from then on. `kebbie hub devices` lists every device with whether it waits, is approved or is denied.

- If you leave it, `hub.auto_approve = "24h"` approves the device 24 hours after its first command; a command waits at most its hour, so send it again then.
- If you press `x` or run `kebbie hub devices deny`, the device stays view only on this machine.
- If you set `hub.trust = "account"`, every device signed in to your account makes changes at once.

### Approve a new device from another device

A device you already approved on the machine can approve or deny a new one too, with [`hub.approve_from`](../reference/configuration.md#machine) at its default `"devices"`.

1. The approved device gets the notification `iPhone 16 wants to make changes on MacBook`, or opens the waiting device on the board.
2. Check that the key it shows matches the one under **Account** on the new device.
3. Tap to approve, or deny it.

The machine takes the decision only when the id and the key match a device that still waits, and lets in what that device held. A waiting device never decides on another. With `hub.approve_from = "machine"`, only `N` on the board and `kebbie hub devices` decide.

## Notifications

Each machine sends your devices a notification through the hub when something needs you or moves, sealed to each device so the hub cannot read it. Turn notifications on for the device in the hub's web app and pick which kinds you hear; update the hub first.

| Kind | Title | Text |
| --- | --- | --- |
| A task's question | `checkout · shop` | `Retry a declined card once or not at all? The lead recommends once: after a minute, then give up.` |
| An agent's prompt | `wt3 · kebbie` | `Claude asks to run cargo test --workspace. Yes or No.` |
| The lead's questions on an idea | the item's title | its first question that waits on you, with `(1 of 3)` when there are more |
| A new device | `iPhone 16 wants to make changes on MacBook` | `Added 14:02. Approve or deny it in kebbie: N on the board.` |
| Ready to push | `cache · api is ready to push` | what the change does, its commits and the review's word |
| CI failed | `CI failed on web · app` | the failed job and step, and what kebbie did about it |
| Started, off unless you turn it on | `Started: Retry flaky red runs` | `On MacBook as retry, recipe feature.` |
| A start you asked for that failed, under Started | `Not started: Retry flaky red runs` | why, and that it stays in the backlog |

Tapping a notification opens the task, the item or the device it is about.

- A device hears only what a machine approved it for: under `hub.trust = "device"`, once it is approved there.
- A newer notification about the same task, item or device replaces the older one, and one about something settled elsewhere, a question answered, a task pushed, CI green again, a device decided, an item started, goes with the next notification that device gets.
- A device never hears of what it did itself, such as an item it started.
- A notification the hub cannot take at that moment is sent again on the keeper's next rounds, for up to an hour.
- If nothing arrives, `notify.log` in the state directory says why.

To hear on the machine itself, or through a service of your own, set a [notify command](get-notifications.md).

## Check that it works

```bash
kebbie hub status
```

It shows the hub, the machine's name, the account whose key the machine pinned with the key's two fingerprints, and when the board last went out.

- The board's `H` window shows the machine on its hub with a dot: green with when the board last went out, red with the reason when it does not.
- `kebbie doctor` has a `hub` line: connected, reachable, and the last post. A hub identity file it cannot read fails that line with the file's path.

The keeper posts the board when it changes and once a minute while nothing does, so the phone can tell that the machine is awake. A machine not heard from for three minutes shows as asleep. The hub keeps a board a day at most, so the once-a-minute post keeps an awake machine's board fresh, and a machine that has slept longer shows as asleep with when it was last seen.

## When the hub has not enabled answering

A hub can leave answering off for an account. It still takes the machine's board, so the phone keeps showing it.

A hub that refuses the board answers so, and kebbie then says `This hub has not enabled answering for your account:` with the hub's link: in a line on the board, in the `H` window, in `kebbie hub status` and in `kebbie doctor`. Everything on the machine keeps working, and the board goes out again with the next post once the hub enables answering for the account.

## Scripts: join with a code

For a setup without a browser, a hub you run yourself mints a join code for a machine with `kebbie-hub admin join-code`. Then, on the machine:

```bash
kebbie hub join https://hub.example.com <join code>
```

It names the machine as `kebbie hub connect` does, and a join code works once. A machine that joined this way belongs to no account, so it holds no account key and no browser reads its board.

## When the hub moves

kebbie follows the hub to another port or path on its own host. When it moves to another address, kebbie refuses to follow by itself, and `kebbie hub status` shows where the hub points.

1. Check that address.
2. Move to it:

   ```bash
   kebbie hub move https://hub.example.net
   ```

kebbie checks only that the address answers as a hub, then sends the machine's token there with every post.

> **Warning:** Whether it is your hub is yours to check: move only to a hub you run or trust.

## Leave the hub

```bash
kebbie hub leave
```

Or press `l` in the board's `H` window, then `y`. It tells the hub, which removes the machine and its board, so your phone no longer lists it. Then it forgets the keys, the token and the account key, and posts no more boards.

A hub too old to remove a machine that leaves, or one kebbie can't reach, keeps a row for the machine, shown as asleep. kebbie says so, and the hub's admin removes it with `kebbie-hub admin remove-machine <id>`.

Every command: [The hub](../reference/commands.md#the-hub).

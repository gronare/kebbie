# See your board on your phone

Follow your tasks from your phone through a [kebbie hub](https://github.com/gronare/kebbie-hub): every task grouped as on the board, the questions waiting for you with their options, and a notification when a task needs you, asks, is ready to push or its CI fails. The phone reads; you still answer and push on the machine.

Everything the machine sends is end-to-end encrypted to the phones it paired with and signed by the machine. The hub relays it without being able to read it, and nothing the hub sends can start anything on the machine. The board carries task names, repo names, summaries and questions, never a local path, a host name or a dev server address.

You need:

- a browser where you can sign in to the hub with a passkey, or make an account there;
- the keeper running on the machine (`kebbie doctor` says so); it posts the board. With `[keeper] enabled = false`, an open board posts it once a minute instead.

## Connect the machine

On the board, press `H`. Or, in your own terminal:

```bash
kebbie hub connect
```

1. Choose the hub: the kebbie hub, at `hub.kebbie.dev`, which is free to connect and read, or your own, a server you run kebbie-hub on. In a terminal `↑` `↓` move the highlight and Enter chooses, `k` and `o` choose at once and `esc` cancels; without a terminal kebbie asks for `k` or `o`, with `k` the answer to Enter. For your own, give its address: kebbie checks that it answers as a kebbie hub over `https` (`http` only for a hub on this machine), says `✓ answers as a kebbie hub`, then saves it as [`[hub] url`](../reference/configuration.md#keys). With `[hub] url` set, kebbie connects there without asking.
2. Name the machine, once. The phone shows it by its one name, [`[machine] name`](../reference/configuration.md#keys), the same name your other machines on the vault see it by. Set already, kebbie uses it; `--name <name>` sets it; otherwise kebbie asks, and Enter keeps the host's short name. It writes the name to config.toml once the hub took it.
3. kebbie shows a card with a QR code of the link, the machine and the hub, a code such as `Z78K-XF53` and how long the code has left. Nothing opens by itself: press Enter to open the link in your browser (not over SSH), and the card's `⏎ open in browser · or scan with your phone` becomes `✓ Opened in your browser`. Or scan the QR code with your phone. In Ghostty, kitty and WezTerm it is in the terminal's own colours; in other terminals it is drawn in braille, and if the camera keeps reading the text beside it instead, frame the code alone or set `KEBBIE_QR=octant` when your terminal draws block characters well. Without a terminal kebbie prints the whole link and opens nothing. Without a browser on this machine, `c` copies the link to paste elsewhere; the link shown under the card stops at its `#`, since what follows is the secret that ties your browser to this machine.
4. On the page, check that it shows the same code, sign in with your passkey or make an account, and approve the machine. With the box to also show its board in that browser ticked, the browser shows the board at once, with no digits to compare: the link carries a secret that never reaches the hub, and the machine admits only the browser that holds it.

kebbie waits up to 10 minutes and ends on a card saying the machine is connected to the hub, or in one line: cancelled in the browser, or the code expired. The machine's keys and the hub's token for it stay in `hub/` in kebbie's state directory, readable by you alone. Nothing prints them.

A config.toml that is a link, say into a dotfiles repo shared by several machines, is never written: kebbie says which line to set there or in a config.toml of this machine's own.

To rename the machine later, change `[machine] name`, or press `n` in the board's `H` window: the keeper renames it on the hub with its next post, and `kebbie hub status` does it at once. Your phone shows the new name when it next reads the board, unless you gave the machine a name of your own on the phone when you paired it. While the hub is unreachable, `kebbie hub status` and `kebbie doctor` show the name the hub still has and why the rename failed. With `[machine] name` unset the hub keeps the name the machine connected with, even when the host is renamed.

## Add a phone

In your own terminal:

```bash
kebbie hub add-device
```

1. kebbie shows a link and its QR code. Scan it with the phone, sign in with your passkey or make an account, and add the phone. The page proposes a name for the device from its browser, such as `iPhone` or `Safari on Mac`; change it there if you like. That is the name kebbie shows for it.
2. The phone and the machine both show six digits. Answer `y` on the machine only when they are the same. The phone's own tap goes through the hub, so only your `y` admits it; anything else adds nothing, and you start again.

Within a few seconds the phone shows the board. Add each phone or browser the same way; one add-device adds one.

To keep the hub on the phone's home screen:

- iPhone: in Safari, tap Share, then **Add to Home Screen**. Open kebbie from the home screen from now on: iOS delivers notifications only to an app on the home screen.
- Android: in Chrome, open the menu and tap **Add to Home screen** (or **Install app**).

## Turn on notifications

In the app's **Maskiner** (machines) tab, tap **Slå på notiser** (turn on notifications) and allow them. kebbie notifies once for each moment a task needs you: a new question, a task that needs you for something else, a head ready to push, a failed CI run. What already waited when you paired is on the board, not in a notification.

## Check that it works

```bash
kebbie hub status
```

It shows the hub, the machine's name, each device by name with when it was paired, when it last read the board and whether it has notifications on, and when the board last went out. The board's `H` window shows the machine on its hub with a dot, green with when the board last went out and red with the reason when it does not, and `kebbie doctor` has a `hub` line: connected, reachable, and the last post. The keeper posts the board when it changes and once a minute while nothing does, so the phone can tell that the machine is awake; a machine not heard from for three minutes shows as sleeping.

## When the subscription lapses

On a hub with plans, such as the kebbie hub, the hub stops taking the machine's board while the account's subscription is not active. kebbie then says `Your kebbie hub subscription isn't active:` with the link to start it: in a line on the board, in the `H` window, in `kebbie hub status` and in `kebbie doctor`. Everything on the machine keeps working, and the phone gets the board again with the next post once the subscription is active.

## Scripts: join with a code

A hub you run yourself mints a join code for a machine with `kebbie-hub admin join-code`, for a setup without a browser:

```bash
kebbie hub join https://hub.example.com <join code>
```

It names the machine as `kebbie hub connect` does, and a join code works once. A machine that joined this way belongs to no account. Add a phone with `kebbie hub add-device`, or with `kebbie hub pair`, which prints a code to type in the hub's app instead of a link.

## When the hub moves

kebbie follows the hub to another port or path on its own host. When it moves to another address, kebbie refuses to follow by itself and `kebbie hub status` shows where the hub points. Check that address, then:

```bash
kebbie hub move https://hub.example.net
```

kebbie checks only that the address answers as a hub, then sends the machine's token there with every post. Whether it is your hub is yours to check: move only to a hub you run or trust.

## Remove a device

```bash
kebbie hub remove-device iPhone
```

It takes the name `kebbie hub status` shows, or the device's id. The device stops seeing this machine's board, and this machine stops sealing the board to it; it keeps seeing your other machines' boards.

## Leave the hub

```bash
kebbie hub leave
```

Or `l` in the board's `H` window, then `y`. It takes each device off this machine's board and forgets the keys and the token. The hub keeps a row for the machine until its admin removes it.

Every command: [The hub](../reference/commands.md#the-hub).

# See your board on your phone

Follow your tasks from your phone through a [kelpie hub](https://github.com/gronare/kelpie-hub): every task grouped as on the board, the questions waiting for you with their options, and a notification when a task needs you, asks, is ready to push or its CI fails. The phone reads; you still answer and push on the machine.

Everything the machine sends is end-to-end encrypted to the phones it paired with and signed by the machine. The hub relays it without being able to read it, and nothing the hub sends can start anything on the machine. The board carries task names, repo names, summaries and questions, never a local path, a host name or a dev server address.

You need:

- a browser where you can sign in to the hub with a passkey, or make an account there;
- the keeper running on the machine (`kelpie doctor` says so); it posts the board. With `[keeper] enabled = false`, an open board posts it once a minute instead.

## Connect the machine

On the board, press `H`. Or, in your own terminal:

```bash
kelpie hub connect
```

1. Choose the hub. The kelpie hub, at `https://kelpie.gronare.com`, is the default. To use a hub you run yourself, choose your own and give its address: kelpie checks that it answers as a kelpie hub over `https` (`http` only for a hub on this machine), then saves it as [`[hub] url`](../reference/configuration.md#keys). With `[hub] url` set, kelpie connects there without asking.
2. Name the machine, once. The phone shows it by its one name, [`[machine] name`](../reference/configuration.md#keys), the same name your other machines on the vault see it by. Set already, kelpie uses it; `--name <name>` sets it; otherwise kelpie asks, with the host's short name as the answer to Enter, and writes it to config.toml once the hub took it.
3. kelpie prints a link, a code such as `K7QF-M2XD` and a QR code of the link, and opens the link in your browser (not over SSH). Open it on the computer, or scan the QR code with your phone.
4. On the page, check that it shows the same code, sign in with your passkey or make an account, and approve the machine. With the box to also show its board in that browser ticked, the browser shows the board at once, with no digits to compare: the link carries a secret that never reaches the hub, and the machine admits only the browser that holds it.

kelpie waits up to 10 minutes and ends in one line: connected, cancelled in the browser, or the code expired. The machine's keys and the hub's token for it stay in `hub/` in kelpie's state directory, readable by you alone. Nothing prints them.

A config.toml that is a link, say into a dotfiles repo shared by several machines, is never written: kelpie says which line to set there or in a config.toml of this machine's own.

To rename the machine later, change `[machine] name`: the keeper renames it on the hub with its next post, and `kelpie hub status` does it at once. Your phone shows the new name when it next reads the board, unless you gave the machine a name of your own on the phone when you paired it. While the hub is unreachable, `kelpie hub status` and `kelpie doctor` show the name the hub still has and why the rename failed. With `[machine] name` unset the hub keeps the name the machine connected with, even when the host is renamed.

## Add a phone

On the board, press `H`, then `a`. Or, in your own terminal:

```bash
kelpie hub add-device
```

1. kelpie shows a link and its QR code. Scan it with the phone, sign in with your passkey or make an account, and add the phone. The page proposes a name for the device from its browser, such as `iPhone` or `Safari on Mac`; change it there if you like. That is the name kelpie shows for it.
2. The phone and the machine both show six digits. Answer `y` on the machine only when they are the same. The phone's own tap goes through the hub, so only your `y` admits it; anything else adds nothing, and you start again.

Within a few seconds the phone shows the board. Add each phone or browser the same way; one add-device adds one.

To keep the hub on the phone's home screen:

- iPhone: in Safari, tap Share, then **Add to Home Screen**. Open kelpie from the home screen from now on: iOS delivers notifications only to an app on the home screen.
- Android: in Chrome, open the menu and tap **Add to Home screen** (or **Install app**).

## Turn on notifications

In the app's **Maskiner** (machines) tab, tap **Slå på notiser** (turn on notifications) and allow them. kelpie notifies once for each moment a task needs you: a new question, a task that needs you for something else, a head ready to push, a failed CI run. What already waited when you paired is on the board, not in a notification.

## Check that it works

```bash
kelpie hub status
```

It shows the hub, the machine's name, each device by name with when it was paired, when it last read the board and whether it has notifications on, and when the board last went out. The board's `H` window shows the same, its dot green while the board goes out and red with the reason when it does not, and `kelpie doctor` has a `hub` line: connected, reachable, and the last post. The keeper posts the board when it changes and once a minute while nothing does, so the phone can tell that the machine is awake; a machine not heard from for three minutes shows as sleeping.

## When the subscription lapses

On a hub with plans, such as the kelpie hub, the hub stops taking the machine's board while the account's subscription is not active. kelpie then says `Your kelpie hub subscription isn't active:` with the link to start it: in a line on the board, in the `H` window, in `kelpie hub status` and in `kelpie doctor`. Everything on the machine keeps working, and the phone gets the board again with the next post once the subscription is active.

## Scripts: join with a code

A hub you run yourself mints a join code for a machine with `kelpie-hub admin join-code`, for a setup without a browser:

```bash
kelpie hub join https://hub.example.com <join code>
```

It names the machine as `kelpie hub connect` does, and a join code works once. A machine that joined this way belongs to no account. Add a phone with `kelpie hub add-device`, or with `kelpie hub pair`, which prints a code to type in the hub's app instead of a link.

## When the hub moves

kelpie follows the hub to another port or path on its own host. When it moves to another address, kelpie refuses to follow by itself and `kelpie hub status` shows where the hub points. Check that address, then:

```bash
kelpie hub move https://hub.example.net
```

kelpie checks only that the address answers as a hub, then sends the machine's token there with every post. Whether it is your hub is yours to check: move only to a hub you run or trust.

## Remove a device

```bash
kelpie hub remove-device iPhone
```

Or `r` in the board's `H` window: pick the device with `↑` `↓` and press `y`. It takes the name `kelpie hub status` shows, or the device's id. The device stops seeing this machine's board, and this machine stops sealing the board to it; it keeps seeing your other machines' boards.

## Disconnect this machine

```bash
kelpie hub leave
```

Or `l` in the board's `H` window. It takes each device off this machine's board and forgets the keys and the token. The hub keeps a row for the machine until its admin removes it.

Every command: [The hub](../reference/commands.md#the-hub).

# See your board on your phone

Follow your tasks from your phone through a [kelpie hub](https://github.com/gronare/kelpie-hub): every task grouped as on the board, the questions waiting for you with their options, and a notification when a task needs you, asks, is ready to push or its CI fails. The phone reads; you still answer and push on the machine.

Everything the machine sends is end-to-end encrypted to the phones it paired with and signed by the machine. The hub relays it without being able to read it, and nothing the hub sends can start anything on the machine. The board carries task names, repo names, summaries and questions, never a local path, a host name or a dev server address.

You need:

- the address of a hub, here `https://hub.example.com`, and an account on it;
- a join code for this machine, from whoever runs the hub: `kelpie-hub admin join-code`;
- the keeper running on the machine (`kelpie doctor` says so); it posts the board. With `[keeper] enabled = false`, an open board posts it once a minute instead.

## Join the hub

```bash
kelpie hub join https://hub.example.com <join code>
```

The machine makes its keys and keeps them, with the token the hub gave it, in `hub/` in kelpie's state directory, readable by you alone. Nothing prints them. A join code works once.

The phone shows the machine by its one name, [`[machine] name`](../reference/configuration.md#keys), the same name your other machines on the vault see it by:

- set already, the join uses it;
- `--name <name>` sets it in config.toml and joins with it;
- neither, the join asks once in your terminal, `Name this machine (shown on your phone and to your other machines) [<host>]:`, with the host's short name as the answer to Enter. Run without a terminal, it joins with the host's short name and says where to set a name of your own.

When the join writes the name, it says so in one line: `[machine] name = "<name>"` and the file. A config.toml that is a link, say into a dotfiles repo shared by several machines, is never written; the join says to set the name there or in a config.toml of this machine's own.

To rename the machine later, change `[machine] name`: the keeper renames it on the hub with its next post, and `kelpie hub status` does it at once. Your phone shows the new name when it next reads the board, unless you gave the machine a name of your own on the phone when you paired it. While the hub is unreachable, `kelpie hub status` and `kelpie doctor` show the name the hub still has and why the rename failed. With `[machine] name` unset the hub keeps the name the machine joined with, even when the host is renamed.

## Add the app to your home screen

Open `https://hub.example.com` on the phone and sign in.

- iPhone: in Safari, tap Share, then **Add to Home Screen**. Open kelpie from the home screen from now on: iOS delivers notifications only to an app on the home screen.
- Android: in Chrome, open the menu and tap **Add to Home screen** (or **Install app**).

## Pair the phone

1. On the machine, in your own terminal:

   ```bash
   kelpie hub pair
   ```

   It prints a code and waits up to 10 minutes.

2. In the app, tap **Koppla maskin** (pair a machine) and type the code.
3. Both show six digits. Answer `y` on the machine only when they are the same; anything else pairs nothing, and you start again with `kelpie hub pair`.

Within a few seconds the phone shows the board. Pair each phone or browser the same way; one `kelpie hub pair` pairs one.

## Turn on notifications

In the app's **Maskiner** (machines) tab, tap **Slå på notiser** (turn on notifications) and allow them. kelpie notifies once for each moment a task needs you: a new question, a task that needs you for something else, a head ready to push, a failed CI run. What already waited when you paired is on the board, not in a notification.

## Check that it works

```bash
kelpie hub status
```

It shows the hub, the machine's name, the paired phones and when the board last went out. `kelpie doctor` has a `hub` line: joined, reachable, and the last post. The keeper posts the board when it changes and once a minute while nothing does, so the phone can tell that the machine is awake; a machine not heard from for three minutes shows as sleeping.

## When the hub moves

kelpie follows the hub to another port or path on its own host. When it moves to another address, kelpie refuses to follow by itself and `kelpie hub status` shows where the hub points. Check that address, then:

```bash
kelpie hub move https://hub.example.net
```

kelpie checks only that the address answers as a hub, then sends the machine's token there with every post. Whether it is your hub is yours to check: move only to a hub you run or trust.

## Stop

```bash
kelpie hub leave
```

It takes each paired phone off this machine's board and forgets the keys and the token. The hub keeps a row for the machine until its admin removes it with `kelpie-hub admin remove-machine <id>`.

Every command: [The hub](../reference/commands.md#the-hub).

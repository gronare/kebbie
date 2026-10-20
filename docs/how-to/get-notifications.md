# Get notifications

Hear about a task that needs you, a task that is ready, a question, or the lead's summary, wherever you are.

1. Set a shell command in `~/.config/kelpie/config.toml`. kelpie runs it with the event in `KELPIE_EVENT` and one line of text in `KELPIE_TEXT` and `$1`.

   To your phone through [ntfy](https://ntfy.sh):

   ```toml
   [notify]
   command = 'curl -s -H "Title: kelpie $KELPIE_EVENT" -d "$KELPIE_TEXT" https://ntfy.sh/my-kelpie-topic'
   ```

   As a macOS notification:

   ```toml
   [notify]
   command = 'osascript -e "display notification (system attribute \"KELPIE_TEXT\") with title \"kelpie\""'
   ```

2. Test it:

   ```bash
   kelpie notify "test"
   ```

3. If nothing arrives, run the command by hand with `KELPIE_TEXT=test`, and read `notify.log` in the state directory.

The command runs detached and is stopped after 10 seconds. A task sends at most one notification per event a minute. Every event and variable: [Notify command](../reference/configuration.md#notify-command).

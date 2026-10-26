# Get notifications

Hear about a task that needs you, a task that is ready, a question, or the lead's summary, wherever you are.

1. Set a shell command under `[notify]` in `~/.config/kebbie/config.toml`. kebbie runs it with the event in `KEBBIE_EVENT` and one line of text in `KEBBIE_TEXT` and `$1`. To your phone through [ntfy](https://ntfy.sh):

   ```toml
   [notify]
   command = 'curl -s -H "Title: kebbie $KEBBIE_EVENT" -d "$KEBBIE_TEXT" https://ntfy.sh/my-kebbie-topic'
   ```

   Or as a macOS notification:

   ```toml
   [notify]
   command = 'osascript -e "display notification (system attribute \"KEBBIE_TEXT\") with title \"kebbie\""'
   ```

2. Send a test. The notification `test` arrives.

   ```bash
   kebbie notify "test"
   ```

If nothing arrives, run the command by hand with `KEBBIE_TEXT=test`, and read `notify.log` in the state directory.

The command runs detached and is stopped after 10 seconds. A task sends at most one notification per event a minute. Every event and variable: [Notify command](../reference/configuration.md#notify-command).

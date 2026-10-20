# Show plan usage on the board

See how much of your Claude plan's five-hour session and weekly limit you have used, in the board's status line.

1. Make kelpie your status line command in `~/.claude/settings.json`:

   ```json
   "statusLine": { "type": "command", "command": "~/.local/share/kelpie/current/target/release/kelpie statusline" }
   ```

   It prints nothing, so Claude Code shows no extra line.

2. Send any Claude session a prompt. The board shows the usage from the next response on.

The numbers come only on a Claude.ai plan, not with an API key, and are as fresh as the last response of any session.

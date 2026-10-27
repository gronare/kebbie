Output docs/assets/clips/update-and-restarts.gif

Set Shell "bash"
Set FontSize 20
Set Width 1200
Set Height 750
Set Padding 12
Set Framerate 12
Set TypingSpeed 45ms
Set CursorBlink false

Hide
Type `exec "$KDEMO_DIR/env.sh" herdr`
Enter
Sleep 4s
Ctrl+B
Type "b"
Sleep 300ms
Ctrl+B
Type "h"
Sleep 500ms
Type "h"
Sleep 800ms
Show

# A newer release is out: the status line says so, U updates.
Sleep 3s
Type "U"
Sleep 3s

# The update downloads the release; then what's new, and the sessions it restarts on the new code.
Hide
Wait+Screen@240s /any other key closes/
Show
Sleep 7s
Escape
Sleep 3s

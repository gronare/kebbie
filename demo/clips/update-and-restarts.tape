Output docs/assets/clips/update-and-restarts.gif

Set Shell "bash"
Set FontSize 14
Set Width 1280
Set Height 720
Set Padding 10
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
Show

# A newer release is out: the status line says so, U updates.
Sleep 3s
Type "U"
Sleep 3s

# The build is cut; the result, with the idle sessions restarted on the new code.
Hide
Wait+Screen@240s /follows the release channel/
Show
Sleep 6s
Escape
Sleep 3s

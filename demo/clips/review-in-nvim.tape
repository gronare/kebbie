Output docs/assets/clips/review-in-nvim.gif

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

# The review split next to vat-rounding's Claude.
Sleep 1.2s
Type "j"
Sleep 800ms
Type "r"
Sleep 2.5s
Enter
Sleep 1.5s

# A comment on a line, typed into Claude's input box.
Type "/yield"
Enter
Sleep 600ms
Type "c"
Sleep 600ms
Type "Show the difference next to old and new."
Sleep 700ms
Enter
Sleep 1.5s

# Mark the file read, then send the comments from Claude's pane.
Type "r"
Sleep 1.5s
Ctrl+B
Type "h"
Sleep 800ms
Enter
Sleep 6s

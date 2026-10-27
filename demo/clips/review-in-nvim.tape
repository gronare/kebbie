Output docs/assets/clips/review-in-nvim.gif

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

# The review split next to vat-rounding's Claude.
Sleep 1.2s
Type "j"
Sleep 800ms
Type "r"
Sleep 2.5s
Enter
Sleep 1.5s

# A comment on a line goes straight to Claude.
Type "/yield"
Enter
Sleep 600ms
Type "c"
Sleep 600ms
Type "Show the difference next to old and new."
Sleep 700ms
Enter

# Claude changes the code and reports ready.
Sleep 7s

# On the board the reviewer checks its commits against your rules.
Ctrl+B
Type " "
Sleep 6s

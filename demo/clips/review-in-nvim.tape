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

# The review split next to vat-rounding's Claude: its steps as tabs, the first one waiting open in nvim.
Sleep 1.2s
Type "j"
Sleep 800ms
Type "r"
Sleep 3s

# Step 1 reads right: a accepts it and moves on to step 2.
Type "a"
Sleep 2.5s
Enter
Sleep 1.5s

# A comment on a line goes into Claude's input box; Enter there sends it.
Type "/yield"
Enter
Sleep 600ms
Type "c"
Sleep 600ms
Type "Show the difference next to old and new."
Sleep 700ms
Enter
Sleep 1.5s
Ctrl+B
Type "h"
Sleep 800ms
Enter

# Claude folds the change into step 2 and reports ready.
Sleep 7s

# On the board: step 1 accepted, step 2 waiting, and the reviewer checks the commits against your rules.
Ctrl+B
Type " "
Sleep 6s

Output docs/assets/clips/push-and-card.gif

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

# checkout-copy is ready to push; its card shows a screenshot.
Sleep 1.2s
Type "j"
Sleep 400ms
Type "j"
Sleep 400ms
Type "j"
Sleep 1.2s
Type "s"
Sleep 4s

# The push window: the card above the commits, then y.
Type "p"
Sleep 3.5s
Type "y"

# Rebased onto main and pushed; kebbie watches CI on the pushed commit.
Sleep 4s
Hide
Wait+Screen@120s /CI running: CI/
Show
Sleep 4s

# Green: the task closes, and the task queued after it starts by itself.
Hide
Wait+Screen@240s /Show in the checkout how far/
Show
Sleep 6s

Output docs/assets/clips/lead.gif

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
Show

# A sentence to the lead, which writes the prompt and starts the task.
Sleep 1.5s
Tab
Sleep 700ms
Type "Add saved carts to the shop"
Sleep 600ms
Enter

# The task works in its own Space; its question about what you want lands on the board.
Wait+Screen@40s /2 options/
Sleep 4s

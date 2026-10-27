Output docs/assets/clips/backlog.gif

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

# Work waits in the Backlog column, by priority.
Sleep 2s
Right
Sleep 1.5s
Type "j"
Sleep 700ms
Type "j"
Sleep 1.5s

# g starts the item as a task, its name suggested from the title.
Type "g"
Sleep 1.5s
Enter
Sleep 1.5s
Left

# Its Claude reads the code and puts a plan to you before writing any.
Wait+Screen@40s /approve the plan/
Sleep 4s

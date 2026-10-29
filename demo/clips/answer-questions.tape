Output docs/assets/clips/answer-questions.gif

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

# The inbox: the question with its options, the cursor on the recommended one.
Sleep 1.5s
Type "i"
Sleep 2.5s
Down
Sleep 1.5s
Up
Sleep 1.2s
Enter

# The answer reaches the task as a message from you: it lists its steps, commits them, reports ready, and the reviewer passes them.
Sleep 8s
Wait+Screen@60s /rules ✓/
Sleep 3s

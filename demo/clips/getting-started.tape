Output docs/assets/clips/getting-started.gif

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

# A new task from the board: a name, then a prompt.
Sleep 1.5s
Type "n"
Sleep 700ms
Type "wishlist"
Sleep 400ms
Enter
Sleep 600ms
Type "Save products for later from the product page"
Sleep 600ms
Enter
Sleep 2s

# Its own worktree, branch and Space, with Claude at work in it.
Type "/wish"
Sleep 600ms
Enter
Sleep 1.5s
Type "g"
Sleep 6s

# Back on the board: it reports ready, and the reviewer checks its commits.
Ctrl+B
Type " "
Wait+Screen@40s /READY TO PUSH/
Sleep 3.5s

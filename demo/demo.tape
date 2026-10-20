Output docs/assets/demo.gif

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

# 1. A sentence to the lead, which starts saved-carts.
Sleep 2s
Tab
Sleep 700ms
Type "Add saved carts to the shop"
Sleep 600ms
Enter
Sleep 9s

# 2. Down the board, one row at a time, to saved-carts.
Ctrl+B
Type "h"
Sleep 600ms
Type "j"
Sleep 1.2s
Type "j"
Sleep 1.2s
Type "j"
Sleep 1.5s
Screenshot docs/assets/board.png
Sleep 1s

# 3. Its options.
Enter
Sleep 1.2s
Down
Sleep 2.5s
Screenshot docs/assets/options.png
Sleep 500ms
Escape
Sleep 2.5s

# 4. Its question, answered in the inbox.
Type "i"
Sleep 1.5s
Shift+Tab
Sleep 150ms
Shift+Tab
Sleep 400ms
Type "j"
Sleep 1.2s
Tab
Sleep 150ms
Tab
Sleep 1.5s
Screenshot docs/assets/question.png
Sleep 1s
Enter
Sleep 2s
Escape
Sleep 3s

# 5. The review split: a change asked for on the line.
Type "r"
Sleep 2.5s
Enter
Sleep 1.5s
Type "/KEEP_DAYS ="
Enter
Sleep 600ms
Type "c"
Sleep 600ms
Type "Keep the cart for 30 days, not 7."
Sleep 700ms
Enter
Sleep 2s
Screenshot docs/assets/review.png
Sleep 500ms
Ctrl+B
Type "h"
Sleep 900ms
Enter

# 6. Claude reads the comment and changes the code.
Sleep 5s

# 7. Back on the board: working, then ready to push.
Ctrl+B
Type " "
Sleep 6.5s

# 8. The diff again, then the card.
Type "r"
Sleep 2s
Type "u"
Sleep 2.5s
Hide
Ctrl+B
Type " "
Sleep 700ms
Escape
Sleep 300ms
Show
Sleep 700ms
Type "s"
Sleep 3.5s
Screenshot docs/assets/card.png
Sleep 500ms
Type "f"
Sleep 3s
Escape
Sleep 500ms

# 9. Push: the card above the commits, then y.
Type "p"
Sleep 4s
Screenshot docs/assets/push.png
Sleep 500ms
Type "y"

# 10. Pushed: gone from the board, the messages say so.
Sleep 3s
Screenshot docs/assets/pushed.png
Sleep 500ms
Escape
Sleep 3.5s

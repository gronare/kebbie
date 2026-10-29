Output docs/assets/demo.gif

Set Shell "bash"
Set FontSize 18
Set Width 1440
Set Height 810
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

# 1. A sentence to the lead, which starts saved-carts; its question lands on the board.
Sleep 2s
Tab
Sleep 700ms
Type "Add saved carts to the shop"
Sleep 600ms
Enter
Wait+Screen@40s /2 options/
Sleep 1.5s

# 2. The board: every task grouped by what it needs from you.
Ctrl+B
Type "h"
Sleep 2s
Screenshot docs/assets/board.png
Sleep 1s

# 3. saved-carts' question, answered with the recommended option; the task lists its steps and commits them one by one.
Type "/saved"
Sleep 400ms
Enter
Sleep 800ms
Type "s"
Sleep 2s
Screenshot docs/assets/question.png
Sleep 1s
Enter
Sleep 1.5s
Escape
Sleep 8s

# 4. The review: the steps as tabs, the first one waiting for you open in nvim, and a change asked for on the line.
Type "r"
Sleep 3s
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

# 5. Claude folds the change into the step it belongs to.
Sleep 7s

# 6. Each step accepted in the review, one at a time.
Ctrl+B
Type " "
Sleep 1.5s
Type "r"
Sleep 2s
Type "l"
Sleep 1.2s
Type "a"
Sleep 1.5s
Type "a"
Sleep 2s

# 7. Back on the board: both steps accepted; its options, then the card with its screenshot.
Ctrl+B
Type " "
Sleep 3s
Enter
Sleep 1.5s
Screenshot docs/assets/options.png
Sleep 1s
Escape
Sleep 500ms
Type "s"
Sleep 3.5s
Screenshot docs/assets/card.png
Sleep 500ms
Type "f"
Sleep 3s
Escape
Sleep 500ms

# 8. Push: the card above the commits, both accepted, then y.
Type "p"
Sleep 3.5s
Screenshot docs/assets/push.png
Sleep 500ms
Type "y"

# 9. Rebased onto main and pushed; kebbie watches its CI.
Sleep 5s
Screenshot docs/assets/pushed.png
Sleep 3s

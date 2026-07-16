Output docs/demo-cached-plan.gif

Set Shell "zsh"
Set FontSize 14
Set Width 1200
Set Height 720
Set Theme "Dracula"
Set Framerate 24
Set TypingSpeed 40ms

# Hidden setup: launch and move the cursor to platform/deep-tree
Hide
Type `alias rug="$PWD/target/release/rug"; clear`
Enter
Type "rug --dir fixtures/"
Enter
Sleep 2s
Type "jjjjjjjjj"
Sleep 300ms
Show

# Plan the highlighted module
Sleep 1s
Type "p"
Wait+Screen@30s /✓1/
Sleep 2s

# Back on Select: the row now carries a green P:{age} badge — a saved plan
# file, ready to apply
Escape
Sleep 3s

# Apply: the confirm dialog shows exactly which plan will be used
Type "a"
Sleep 3s
Type "y"

# The apply consumes the cached plan file — no re-plan
Wait+Screen@30s /Apply complete/
Sleep 3s

Hide
Type "q"
Type "q"

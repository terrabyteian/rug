Output docs/demo-targeted.gif

Set Shell "zsh"
Set FontSize 14
Set Width 1200
Set Height 720
Set Theme "Dracula"
Set Framerate 24
Set TypingSpeed 40ms

# Hidden setup: launch, cursor to platform/deep-tree, open the explorer
Hide
Type `alias rug="$PWD/target/release/rug"; clear`
Enter
Type "rug --dir fixtures/"
Enter
Sleep 2s
Type "jjjjjjjjj"
Sleep 300ms
Type "s"
Sleep 2.5s
Show

# Walk down to the module.networking header row
Sleep 1s
Type "jjjjjjjjjjjjjj"
Sleep 1s

# Space on a header selects the whole module as one -target
Space
Sleep 2s

# Queue a targeted plan for it
Type "p"
Sleep 2.5s

# Dismiss the notice, close the explorer, jump to the task board
Type "j"
Sleep 500ms
Escape
Sleep 1s
Tab
Wait+Screen@30s /✓1/
Sleep 2.5s

# Apply: the confirm warns the cached plan is targeted and lists exactly
# what it covers
Type "A"
Sleep 3.5s
Type "y"
Wait+Screen@30s /Apply complete/
Sleep 3s

Hide
Type "q"
Type "q"

Output docs/demo-explorer.gif

Set Shell "zsh"
Set FontSize 14
Set Width 1200
Set Height 720
Set Theme "Dracula"
Set Framerate 24
Set TypingSpeed 40ms

# Hidden setup: launch and move the cursor to services/big-state
Hide
Type `alias rug="$PWD/target/release/rug"; clear`
Enter
Type "rug --dir fixtures/"
Enter
Sleep 2s
Type "jjjjjjjjjjjjjjjjj"
Sleep 300ms
Show

# Open the state explorer for the highlighted module
Sleep 1s
Type "s"
Sleep 2.5s

# Browse: resources grouped by child module, tainted resources flagged
Type "j"
Sleep 250ms
Type "j"
Sleep 250ms
Type "j"
Sleep 250ms
Type "j"
Sleep 250ms
Type "j"
Sleep 250ms
Type "j"
Sleep 1s

# Filter by address
Type "/"
Sleep 400ms
Type@200ms "networking"
Sleep 2s
Escape
Sleep 1s

# Inspect a resource's attributes (cursor to the top row off-camera)
Hide
Type "kkkkkkkkkkkkkkkkkkkk"
Sleep 300ms
Show
Enter
Sleep 3s
Type "j"
Sleep 250ms
Type "j"
Sleep 250ms
Type "j"
Sleep 1.5s
Escape
Sleep 1.5s

Hide
Escape
Sleep 300ms
Type "q"
Type "q"

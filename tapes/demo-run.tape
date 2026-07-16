Output docs/demo-run.gif

Set Shell "zsh"
Set FontSize 14
Set Width 1200
Set Height 720
Set Theme "Dracula"
Set Framerate 24
Set TypingSpeed 40ms

# Hidden setup: launch, filter to services/, mark slow-plan, slow-plan-2
# and verbose-deploy
Hide
Type `alias rug="$PWD/target/release/rug"; clear`
Enter
Type "rug --dir fixtures/"
Enter
Sleep 2s
Type "/"
Sleep 200ms
Type "services"
Sleep 300ms
Enter
Sleep 300ms
Type "jjjj"
Sleep 200ms
Space
Sleep 100ms
Type "j"
Sleep 100ms
Space
Sleep 100ms
Type "j"
Sleep 100ms
Space
Sleep 300ms
Show

# Three modules marked — plan them
Sleep 1s
Type "p"

# The status board: three tasks running at once, output streaming for the
# highlighted module
Sleep 4s

# Move the cursor — the output pane follows
Type "j"
Sleep 1.5s
Type "j"
Sleep 2.5s

# Fullscreen the streaming output
Enter
Sleep 3s
Escape
Sleep 1s

# slow-plan-2 runs ~40s — mark it and cancel just that task
Type "k"
Sleep 800ms
Space
Sleep 600ms
Type "C"
Sleep 2s
Type "y"

# Show the graceful shutdown starting (SIGINT, "Interrupt received")
Sleep 2.5s

# Hidden: wait out the rest of the graceful shutdown
Hide
Wait+Screen@90s /cancelled/
Sleep 200ms
Show
Sleep 1s

# Final board: two done, one cancelled
Type "k"
Sleep 3s

Hide
Type "q"
Type "q"

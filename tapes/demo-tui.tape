Output docs/demo-tui.gif

Set Shell "zsh"
Set FontSize 14
Set Width 1100
Set Height 650
Set Theme "Dracula"
Set Framerate 24
Set TypingSpeed 40ms

# Launch rug from the project directory
Type "cd /Users/ian/projects/rug && ./target/release/rug --dir fixtures/"
Enter
Sleep 1.5s

# Browse the module list on the Select screen
Type "j"
Sleep 150ms
Type "j"
Sleep 150ms
Type "j"
Sleep 150ms
Type "j"
Sleep 300ms

# Jump to the bottom of the list
Type "G"
Sleep 600ms

# Skip slow-plan-2 (takes ~40s) — land on slow-plan (~10s streaming plan)
Type "k"
Sleep 200ms
Type "k"
Sleep 400ms

# Enter opens the Run screen for the highlighted module
Enter
Sleep 700ms

# Plan it — the status board and live output appear automatically
Type "p"
Sleep 13s

# Esc hops back to Select while the session keeps running, to show the
# two-screen relationship
Escape
Sleep 1.2s

# Quit — the plan has already finished, so a single q exits directly
Type "q"

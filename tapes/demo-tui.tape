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

# Browse the module list
Type "j"
Sleep 150ms
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

# Skip slow-plan-2 (takes 40s) — land on slow-plan (10s) and select it
Type "k"
Sleep 200ms
Type "k"
Sleep 200ms
Space
Sleep 500ms

# Plan the selected module
Type "p"
Sleep 800ms

# Cycle to Tasks pane
Tab
Sleep 800ms

# Cycle to Output pane — watch live streaming plan output
Tab
Sleep 12s

# Quit gracefully
Type "q"

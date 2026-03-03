Output docs/demo-filter.gif

Set Shell "zsh"
Set FontSize 14
Set Width 1100
Set Height 650
Set Theme "Dracula"
Set Framerate 24
Set TypingSpeed 40ms

# Launch rug
Type "cd /Users/ian/projects/rug && ./target/release/rug --dir fixtures/"
Enter
Sleep 1.5s

# Open filter bar
Type "/"
Sleep 300ms

# Type filter — list narrows to apps/api and apps/web
Type "apps"
Sleep 800ms

# Select apps/api (already highlighted at top of filtered list)
Space
Sleep 200ms

# Move to apps/web and select it
Type "j"
Sleep 200ms
Space
Sleep 500ms

# Clear filter — full list returns, two modules remain highlighted
Escape
Sleep 800ms

# Plan the two selected modules
Type "p"
Sleep 3s

# Quit
Type "q"

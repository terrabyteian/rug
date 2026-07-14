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

# Open the filter bar
Type "/"
Sleep 300ms

# Type filter — list narrows to apps/api and apps/web
Type "apps"
Sleep 800ms

# Commit the filter: Enter exits input mode but keeps it applied (typing
# Space/j next must NOT land in the filter text)
Enter
Sleep 500ms

# Select apps/api (highlighted at top of the filtered list)
Space
Sleep 300ms

# Move to apps/web and select it too
Type "j"
Sleep 200ms
Space
Sleep 500ms

# Clear the applied filter — full list returns, both modules stay selected
Escape
Sleep 800ms

# Cross into the Run screen and plan both selected modules
Type "p"
Sleep 4s

# Quit — both plans finish almost instantly, so a single q exits directly
Type "q"

Output docs/demo-select.gif

Set Shell "zsh"
Set FontSize 14
Set Width 1200
Set Height 720
Set Theme "Dracula"
Set Framerate 24
Set TypingSpeed 40ms

# Hidden setup + launch
Hide
Type `alias rug="$PWD/target/release/rug"; clear`
Enter
Type "rug --dir fixtures/"
Enter
Sleep 2s
Show

Sleep 1s

# Filter: the list narrows live as you type
Type "/"
Sleep 500ms
Type@250ms "apps"
Sleep 1s

# Enter keeps the filter applied
Enter
Sleep 600ms

# Mark both filtered modules
Space
Sleep 400ms
Type "j"
Sleep 300ms
Space
Sleep 800ms

# Esc clears the filter — the full list returns, marks persist
Escape
Sleep 1.5s

# Fold the tree by depth with [ and ]
Type "["
Sleep 900ms
Type "["
Sleep 1.2s
Type "]"
Sleep 900ms
Type "]"
Sleep 1.2s

# Select everything visible
Type "*"
Sleep 2.5s

Hide
Type "q"
Type "q"

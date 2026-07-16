Output docs/demo-hero.gif

Set Shell "zsh"
Set FontSize 14
Set Width 1200
Set Height 720
Set Theme "Dracula"
Set Framerate 24
Set TypingSpeed 40ms

# Hidden setup: alias so the visible command is just `rug`
Hide
Type `alias rug="$PWD/target/release/rug"; clear`
Enter
Show

# Launch rug against the fixture tree
Type "rug --dir fixtures/"
Enter
Sleep 2s

# Hidden: select everything, then unmark the modules that can't plan here
# (remote-state/demo needs MinIO, big-state has no sources by design) and
# the two deliberately slow ones, then return the cursor to the top.
Hide
Type "*"
Sleep 200ms
Type "jjjjjjjjjjjjjj"
Sleep 200ms
Space
Sleep 100ms
Type "jjj"
Sleep 100ms
Space
Sleep 100ms
Type "jj"
Sleep 100ms
Space
Sleep 100ms
Type "j"
Sleep 100ms
Space
Sleep 100ms
Type "g"
Sleep 300ms
Show

# The whole tree is marked — kick off plans across all of it
Sleep 1.2s
Type "p"

# Run screen: 4 tasks at a time cascade through 18 modules
Wait+Screen@60s /✓18/
Sleep 2.5s

# Back on Select: every module now carries a cached-plan badge
Escape
Sleep 3s

Hide
Type "q"
Type "q"

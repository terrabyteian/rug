Output docs/demo-headless.gif

Set Shell "zsh"
Set FontSize 14
Set Width 1200
Set Height 720
Set Theme "Dracula"
Set Framerate 24
Set TypingSpeed 50ms

# Hidden setup: alias so the visible command is just `rug`
Hide
Type `alias rug="$PWD/target/release/rug"; clear`
Enter
Show

# List the discovered modules
Type "rug --dir fixtures/ list"
Enter
Sleep 3s

# Plan a subtree headlessly — output streams with a per-module prefix
Type "rug --dir fixtures/ plan --filter platform"
Enter
Sleep 14s

# Exit code for CI
Type "echo $?"
Enter
Sleep 2.5s

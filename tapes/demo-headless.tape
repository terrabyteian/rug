Output docs/demo-headless.gif

Set Shell "zsh"
Set FontSize 14
Set Width 1000
Set Height 480
Set Theme "Dracula"
Set Framerate 24
Set TypingSpeed 60ms

# Move to project root and run headless plan
Type "cd /Users/ian/projects/rug && ./target/release/rug --dir fixtures/ plan --filter infra"
Enter
Sleep 6s

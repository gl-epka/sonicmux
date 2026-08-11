Output docs/assets/sonicmux-tui.gif

Set Shell zsh
Set FontSize 16
Set Width 1200
Set Height 720
Set Padding 18
Set Margin 20
Set MarginFill "#11111b"
Set BorderRadius 10
Set Framerate 24
Set LoopOffset 10%
Set TypingSpeed 20ms
Set CursorBlink false
Set Theme "Catppuccin Mocha"

Type `target/release/sonicmux-tui --dry-run "$SONICMUX_M6_DEMO_INPUT"`
Enter
Wait+Screen /READY/
Sleep 1s

Type "2"
Sleep 2s

Type "1"
Type "s"
Wait+Screen /PLAN/
Sleep 2s

Type "3"
Sleep 2s

Type "?"
Sleep 2s
Type "q"
Sleep 1s
Type "q"

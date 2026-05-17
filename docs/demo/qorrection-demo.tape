# qorrection demo: q9 wrapping GitHub Copilot CLI; typing `:q` summons the ambulance.
#
# Re-render:
#   1. Build q9: `cargo build --release` from this worktree's root.
#   2. Ensure GitHub Copilot CLI (`copilot`) is on PATH and authenticated.
#   3. From the worktree root, run: `vhs docs/demo/qorrection-demo.tape`.
#
# Notes:
#   - Copilot's reply is non-deterministic; tune the Sleep values per take
#     so the AI reply and the full ambulance pass stay on-screen.
#   - Sanity-check the rendered GIF before committing: no auth tokens,
#     no full home paths, no unrelated terminal history.

Output docs/demo/qorrection-demo.gif

Set Shell bash
Set Theme "Catppuccin Mocha"
Set FontSize 16
Set Width 1000
Set Height 600
Set Padding 16
Set TypingSpeed 60ms

# Hidden setup: minimal prompt + put the freshly built q9 on PATH so the
# visible session can type the bare `q9 copilot --banner`.
Hide
Type "export PS1='$ '"
Enter
Type "export PATH=$PWD/target/release:$PATH"
Enter
Type "clear"
Enter
Sleep 500ms
Show

# 1. Launch q9 wrapping the real Copilot CLI with its splash banner.
Type "q9 copilot --banner"
Enter
Sleep 5s

# 2. Ask Copilot to describe itself in a single tweet.
Type "Describe the features of this CLI app in a single tweet."
Enter
Sleep 15s

# 3. Reflexively `:q` like a Vim user. qorrection's ambulance sweeps in.
Type ":q"
Enter
Sleep 10s


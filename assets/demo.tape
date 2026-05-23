# Re-record with (run from project root):
#
#   cargo build --release --no-default-features
#   vhs assets/demo.tape
#
#   # vhs Hide leaks ~10 setup frames at the start — trim them:
#   python3 -c "
#   from PIL import Image, ImageSequence
#   img = Image.open('assets/demo.gif')
#   frames, durations = [], []
#   for i, f in enumerate(ImageSequence.Iterator(img)):
#       if i < 10: continue
#       frames.append(f.copy()); durations.append(f.info.get('duration', 40))
#   frames[0].save('assets/demo.gif', save_all=True, append_images=frames[1:],
#                  duration=durations, loop=0, optimize=True, disposal=2)
#   "
#
#   # Re-optimize (Pillow's gif is bloated):
#   gifsicle -O3 --colors 256 assets/demo.gif -o assets/demo.gif
#
# Fixtures live under assets/demo-fixtures/ and are env-var-injected so the
# demo never touches the user's real session stores or ~/.ccr/ state.

Output assets/demo.gif

Set Theme "OneDark"
Set FontSize 14
Set Width 1100
Set Height 650
Set Padding 20
Set TypingSpeed 50ms

# ── Hidden setup ──────────────────────────────────────────────
Hide
Type "PATH=$PWD/target/release:$PATH"
Enter
Type "export CCR_CLAUDE_DIR=$PWD/assets/demo-fixtures/claude"
Enter
Type "export CCR_CODEX_DIR=$PWD/assets/demo-fixtures/codex"
Enter
Type "export CCR_GEMINI_DIR=$PWD/assets/demo-fixtures/gemini"
Enter
Type "export CCR_BOOKMARKS_FILE=/tmp/ccr-demo-bookmarks.json"
Enter
Type "export CCR_NICKNAMES_FILE=/tmp/ccr-demo-nicknames.json"
Enter
Type "rm -f $CCR_BOOKMARKS_FILE $CCR_NICKNAMES_FILE"
Enter
Type "clear"
Enter
Show

# ── Visible demo ──────────────────────────────────────────────
Type "ccr"
Sleep 200ms
Enter
Sleep 1500ms

# Navigate the list — preview pane follows
Down
Sleep 900ms
Down
Sleep 900ms
Up
Sleep 700ms
Up
Sleep 700ms

# Set a nickname on the selected (top) session
Type "n"
Sleep 600ms
Type "panic hotfix"
Sleep 1000ms
Enter
Sleep 1300ms

# Filter — `/` hits title, cwd, tool, nickname, and content
Type "/"
Sleep 400ms
Type "panic"
Sleep 1500ms
Enter
Sleep 1500ms

# Clear filter and quit
Escape
Sleep 700ms
Type "q"
Sleep 400ms

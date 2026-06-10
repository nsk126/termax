# termax

A minimal standalone terminal emulator in Rust: it opens its own window,
runs your shell on a PTY, and draws the screen itself.

## How it works

```
key events ──► gui (egui window) ──► pty master ──► shell (child on pty slave)
screen     ◄── gui draws the Grid ◄── vte::Parser ◄── pty master output
```

Every byte the shell emits is fed through a VT escape-sequence parser into
an in-memory screen model (`Grid`): a 2D array of styled cells plus a
cursor. The egui front end draws that grid each frame and encodes key
presses (text, Ctrl-chars, arrows, ...) into the bytes a terminal sends.
This split — I/O, screen model, renderer — is the same architecture
Alacritty and WezTerm use, and the model stays renderer-agnostic: any
front end only needs `Grid::cells` and `Grid::cursor`.

## Layout

- `src/main.rs` — wiring: PTY setup (`portable-pty`), shell spawn, the
  reader thread that pumps PTY output through the parser into the grid.
- `src/term.rs` — `Grid` screen model and the `vte::Perform` implementation:
  printing, line discipline, cursor movement (CUP/CUU/CUD/...), erase
  (ED/EL), SGR colors (16/256/truecolor), alternate screen tracking, resize.
- `src/gui.rs` — the egui front end: grid rendering (styled runs, block
  cursor, xterm 256-color palette), key-to-bytes encoding, window-resize
  to PTY-resize propagation.

## Usage

```sh
cargo run                 # open a termax window running your $SHELL
cargo run -- htop         # run a one-off command instead
cargo test                # unit tests for the screen model
```

Or build with `cargo build --release` and launch `target/release/termax`
directly — from a file manager, a launcher, or another terminal.

## Where to take it next

- Scrollback: keep rows pushed off the top of the grid instead of dropping
  them, plus mouse-wheel scrolling.
- Selection and clipboard copy (paste already works).
- A real font stack: a configurable monospace font with bold/italic faces
  and wide-glyph (CJK, emoji) handling.
- More escape sequences: insert/delete line (IL/DL), scroll regions
  (DECSTBM), cursor save/restore, OSC window title.
- A proper alternate screen: a second grid swapped in/out, restoring the
  primary screen on exit (vim/less currently draw into the same grid).

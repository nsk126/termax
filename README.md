# termax

A minimal terminal emulator core in Rust — the foundation to build a full
emulator on.

## What it does

It spawns a shell on a pseudo-terminal (PTY) and does two things with every
byte the shell emits:

1. Passes it through to your current terminal, so the session is fully
   interactive today.
2. Feeds it through a VT escape-sequence parser into an in-memory screen
   model (`Grid`): a 2D array of styled cells plus a cursor.

```
stdin ──────────────► pty master ──► shell (child on pty slave)
stdout ◄── passthrough ◄── pty master output
                             │
                             └──► vte::Parser ──► term::Grid (screen model)
```

This is the same architecture Alacritty and WezTerm use. The screen model is
renderer-agnostic: replacing the passthrough with a GPU front end (wgpu,
egui, ...) that draws `Grid::cells` each frame turns this into a standalone
GUI terminal. On exit it prints the final grid state to show the model
tracked the session.

## Layout

- `src/main.rs` — PTY setup (`portable-pty`), raw-mode stdin, the two I/O
  pump threads.
- `src/term.rs` — `Grid` screen model and the `vte::Perform` implementation:
  printing, line discipline, cursor movement (CUP/CUU/CUD/...), erase
  (ED/EL), SGR colors (16/256/truecolor), alternate screen tracking.

## Usage

```sh
cargo run                 # run your $SHELL inside termax
cargo run -- ls --color   # run a one-off command
cargo test                # unit tests for the screen model
```

## Where to take it next

- Resize handling: catch `SIGWINCH`, call `master.resize()` and rebuild the grid.
- Scrollback: keep rows pushed off the top of the grid instead of dropping them.
- A real renderer: `winit` + `wgpu` (or `egui`) drawing the grid with a
  monospace font atlas — at that point the passthrough goes away.
- More escape sequences: insert/delete line (IL/DL), scroll regions
  (DECSTBM), cursor save/restore, OSC window title.

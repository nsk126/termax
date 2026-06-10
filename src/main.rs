//! termax — a terminal emulator.
//!
//!   key events ──► gui (egui window) ──► pty master ──► shell (child on pty slave)
//!   screen     ◄── gui draws term::Grid ◄── vte::Parser ◄── pty master output
//!
//! main wires the pieces together: it spawns the shell on a PTY, runs a
//! reader thread that feeds shell output through the VT parser into the
//! Grid (the in-memory screen model), and hands the Grid to the egui
//! front end, which draws it each frame and writes key presses back to
//! the PTY.

mod gui;
mod term;

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::Read;
use std::sync::{Arc, Mutex};

use term::Grid;

const INITIAL_ROWS: u16 = 24;
const INITIAL_COLS: u16 = 80;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Spawn the shell (or the command given as args) on a fresh PTY.
    // The GUI resizes it to the real cell count on the first frame.
    let pty = native_pty_system().openpty(PtySize {
        rows: INITIAL_ROWS,
        cols: INITIAL_COLS,
        pixel_width: 0,
        pixel_height: 0,
    })?;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut cmd = if args.is_empty() {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into());
        CommandBuilder::new(shell)
    } else {
        let mut c = CommandBuilder::new(&args[0]);
        c.args(&args[1..]);
        c
    };
    cmd.env("TERM", "xterm-256color");
    if let Ok(cwd) = std::env::current_dir() {
        cmd.cwd(cwd);
    }

    let child = pty.slave.spawn_command(cmd)?;
    // Drop our handle to the slave end so the reader sees EOF when the
    // child exits.
    drop(pty.slave);

    let grid = Arc::new(Mutex::new(Grid::new(
        INITIAL_ROWS as usize,
        INITIAL_COLS as usize,
    )));

    let mut pty_reader = pty.master.try_clone_reader()?;
    let writer = pty.master.take_writer()?;

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("termax")
            .with_inner_size([960.0, 640.0]),
        ..Default::default()
    };

    let reader_grid = Arc::clone(&grid);
    eframe::run_native(
        "termax",
        options,
        Box::new(move |cc| {
            // Reader: pty output -> vte parser -> grid, waking the UI per
            // chunk so it redraws.
            let ctx = cc.egui_ctx.clone();
            std::thread::spawn(move || {
                let mut parser = vte::Parser::new();
                let mut buf = [0u8; 4096];
                while let Ok(n) = pty_reader.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    {
                        let mut grid = reader_grid.lock().unwrap();
                        for &byte in &buf[..n] {
                            parser.advance(&mut *grid, byte);
                        }
                    }
                    ctx.request_repaint();
                }
                // One last wake so update() notices the child exited.
                ctx.request_repaint();
            });
            Ok(Box::new(gui::TermaxApp::new(grid, pty.master, writer, child)))
        }),
    )?;

    Ok(())
}

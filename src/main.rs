//! termax — minimal terminal emulator core.
//!
//! Architecture (the same shape Alacritty/WezTerm use):
//!
//!   stdin ──────────────► pty master ──► shell (child on pty slave)
//!   stdout ◄── passthrough ◄── pty master output
//!                                │
//!                                └──► vte::Parser ──► term::Grid (screen model)
//!
//! Today the "renderer" is your existing terminal (bytes are passed
//! through verbatim), but every byte is also interpreted into the Grid,
//! so the screen state is fully modeled in memory. Swapping passthrough
//! for a wgpu/egui front end that draws the Grid turns this into a
//! standalone GUI emulator. On exit it prints the Grid's final state to
//! prove the model tracked the session.

mod term;

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use term::Grid;

/// Size of the controlling terminal, or 24x80 when not attached to one.
fn terminal_size() -> (u16, u16) {
    unsafe {
        let mut ws: libc::winsize = std::mem::zeroed();
        if libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) == 0 && ws.ws_col > 0 {
            return (ws.ws_row, ws.ws_col);
        }
    }
    (24, 80)
}

/// Puts stdin into raw mode so keystrokes (including ^C, arrows) reach the
/// child shell instead of being interpreted by the outer terminal; restores
/// the original settings on drop.
struct RawMode(Option<libc::termios>);

impl RawMode {
    fn enable() -> Self {
        unsafe {
            if libc::isatty(libc::STDIN_FILENO) == 1 {
                let mut tio: libc::termios = std::mem::zeroed();
                if libc::tcgetattr(libc::STDIN_FILENO, &mut tio) == 0 {
                    let original = tio;
                    libc::cfmakeraw(&mut tio);
                    libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &tio);
                    return RawMode(Some(original));
                }
            }
        }
        RawMode(None)
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        if let Some(original) = self.0 {
            unsafe {
                libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &original);
            }
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (rows, cols) = terminal_size();

    // Spawn the shell (or the command given as args) on a fresh PTY.
    let pty = native_pty_system().openpty(PtySize {
        rows,
        cols,
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

    let mut child = pty.slave.spawn_command(cmd)?;
    // Drop our handle to the slave end so the reader sees EOF when the
    // child exits.
    drop(pty.slave);

    let grid = Arc::new(Mutex::new(Grid::new(rows as usize, cols as usize)));

    let raw_mode = RawMode::enable();

    // Reader: pty output -> screen passthrough + grid model.
    let reader_grid = Arc::clone(&grid);
    let mut pty_reader = pty.master.try_clone_reader()?;
    let reader = std::thread::spawn(move || {
        let mut parser = vte::Parser::new();
        let mut stdout = std::io::stdout();
        let mut buf = [0u8; 4096];
        while let Ok(n) = pty_reader.read(&mut buf) {
            if n == 0 {
                break;
            }
            let _ = stdout.write_all(&buf[..n]);
            let _ = stdout.flush();
            let mut grid = reader_grid.lock().unwrap();
            for &byte in &buf[..n] {
                parser.advance(&mut *grid, byte);
            }
        }
    });

    // Writer: stdin -> pty. Detached: it blocks on stdin and dies with us.
    let mut pty_writer = pty.master.take_writer()?;
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut buf = [0u8; 1024];
        while let Ok(n) = stdin.read(&mut buf) {
            if n == 0 {
                break;
            }
            if pty_writer.write_all(&buf[..n]).is_err() {
                break;
            }
        }
    });

    let status = child.wait()?;
    let _ = reader.join();
    drop(raw_mode);

    // Demonstrate that the grid tracked the session.
    let grid = grid.lock().unwrap();
    println!("\n--- termax: final screen model ({}x{}) ---", grid.rows, grid.cols);
    println!("{}", grid.to_text());
    println!(
        "--- cursor at row {}, col {}; child exited with {} ---",
        grid.cursor.0,
        grid.cursor.1,
        status.exit_code()
    );

    Ok(())
}

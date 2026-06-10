//! The egui front end: each frame it draws the `Grid` as styled monospace
//! text and encodes key presses into the byte sequences a terminal sends
//! (printable text, control chars, CSI arrow/function sequences) on the
//! PTY writer.

use std::io::Write;
use std::sync::{Arc, Mutex};

use eframe::egui::{
    self, Color32, Event, FontId, Key, Modifiers, Rect, Stroke, TextFormat, Vec2,
    text::LayoutJob,
};
use portable_pty::{Child, MasterPty, PtySize};

use crate::term::{Cell, Color, Grid, Style};

const FONT_SIZE: f32 = 15.0;
const DEFAULT_FG: Color32 = Color32::from_rgb(0xd8, 0xd8, 0xd8);
const DEFAULT_BG: Color32 = Color32::from_rgb(0x14, 0x14, 0x1c);

pub struct TermaxApp {
    grid: Arc<Mutex<Grid>>,
    pty: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
}

impl TermaxApp {
    pub fn new(
        grid: Arc<Mutex<Grid>>,
        pty: Box<dyn MasterPty + Send>,
        writer: Box<dyn Write + Send>,
        child: Box<dyn Child + Send + Sync>,
    ) -> Self {
        TermaxApp { grid, pty, writer, child }
    }

    /// Turn this frame's keyboard events into bytes on the PTY.
    fn send_input(&mut self, ctx: &egui::Context) {
        let mut bytes: Vec<u8> = Vec::new();
        ctx.input(|i| {
            for event in &i.events {
                match event {
                    Event::Text(s) => bytes.extend_from_slice(s.as_bytes()),
                    Event::Paste(s) => bytes.extend_from_slice(s.as_bytes()),
                    Event::Key { key, pressed: true, modifiers, .. } => {
                        if let Some(seq) = encode_key(*key, *modifiers) {
                            bytes.extend_from_slice(&seq);
                        }
                    }
                    _ => {}
                }
            }
        });
        if !bytes.is_empty() {
            let _ = self.writer.write_all(&bytes);
            let _ = self.writer.flush();
        }
    }
}

impl eframe::App for TermaxApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        // Shell exited -> close the window.
        if matches!(self.child.try_wait(), Ok(Some(_))) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        self.send_input(&ctx);

        let font = FontId::monospace(FONT_SIZE);
        let (cell_w, cell_h) =
            ctx.fonts_mut(|f| (f.glyph_width(&font, ' '), f.row_height(&font)));

        let avail = ui.available_size();
        let cols = ((avail.x / cell_w) as usize).max(2);
        let rows = ((avail.y / cell_h) as usize).max(1);

        let mut grid = self.grid.lock().unwrap();
        if (rows, cols) != (grid.rows, grid.cols) {
            grid.resize(rows, cols);
            let _ = self.pty.resize(PtySize {
                rows: rows as u16,
                cols: cols as u16,
                pixel_width: 0,
                pixel_height: 0,
            });
        }

        let origin = ui.min_rect().min;
        let painter = ui.painter();
        painter.rect_filled(ui.max_rect(), 0.0, DEFAULT_BG);
        for (r, row) in grid.cells.iter().enumerate() {
            let galley = ctx.fonts_mut(|f| f.layout_job(layout_row(row, &font)));
            painter.galley(
                origin + Vec2::new(0.0, r as f32 * cell_h),
                galley,
                DEFAULT_FG,
            );
        }

        // Cursor: solid block, with the covered char redrawn in bg color.
        let (r, c) = grid.cursor;
        let c = c.min(grid.cols - 1);
        let cursor_rect = Rect::from_min_size(
            origin + Vec2::new(c as f32 * cell_w, r as f32 * cell_h),
            Vec2::new(cell_w, cell_h),
        );
        painter.rect_filled(cursor_rect, 0.0, DEFAULT_FG);
        let ch = grid.cells[r][c].ch;
        if ch != ' ' {
            painter.text(
                cursor_rect.min,
                egui::Align2::LEFT_TOP,
                ch,
                font.clone(),
                DEFAULT_BG,
            );
        }
    }
}

impl Drop for TermaxApp {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

/// One row of cells as a laid-out line, consecutive same-style cells
/// merged into a single styled section.
fn layout_row(row: &[Cell], font: &FontId) -> LayoutJob {
    let mut job = LayoutJob::default();
    let mut run = String::new();
    let mut run_style: Option<Style> = None;
    for cell in row {
        if run_style != Some(cell.style) {
            if let Some(style) = run_style.take() {
                append_run(&mut job, &run, style, font);
            }
            run.clear();
            run_style = Some(cell.style);
        }
        run.push(cell.ch);
    }
    if let Some(style) = run_style {
        append_run(&mut job, &run, style, font);
    }
    job
}

fn append_run(job: &mut LayoutJob, text: &str, style: Style, font: &FontId) {
    let (fg, bg) = resolve_colors(style);
    job.append(
        text,
        0.0,
        TextFormat {
            font_id: font.clone(),
            color: fg,
            background: bg,
            italics: style.italic,
            underline: if style.underline {
                Stroke::new(1.0, fg)
            } else {
                Stroke::NONE
            },
            ..Default::default()
        },
    );
}

fn resolve_colors(style: Style) -> (Color32, Color32) {
    let mut fg = color32(style.fg, DEFAULT_FG);
    // Bold brightens the 8 base ANSI colors, the classic terminal behavior.
    if style.bold {
        if let Color::Indexed(i @ 0..=7) = style.fg {
            fg = color32(Color::Indexed(i + 8), DEFAULT_FG);
        }
    }
    let bg = match style.bg {
        Color::Default => Color32::TRANSPARENT,
        c => color32(c, DEFAULT_BG),
    };
    if style.inverse {
        let bg = if bg == Color32::TRANSPARENT { DEFAULT_BG } else { bg };
        (bg, fg)
    } else {
        (fg, bg)
    }
}

fn color32(c: Color, default: Color32) -> Color32 {
    match c {
        Color::Default => default,
        Color::Rgb(r, g, b) => Color32::from_rgb(r, g, b),
        Color::Indexed(i) => indexed_color(i),
    }
}

/// The xterm 256-color palette: 16 ANSI colors, a 6x6x6 cube, grayscale.
fn indexed_color(i: u8) -> Color32 {
    const ANSI: [(u8, u8, u8); 16] = [
        (0x00, 0x00, 0x00),
        (0xcd, 0x00, 0x00),
        (0x00, 0xcd, 0x00),
        (0xcd, 0xcd, 0x00),
        (0x2e, 0x6c, 0xff),
        (0xcd, 0x00, 0xcd),
        (0x00, 0xcd, 0xcd),
        (0xe5, 0xe5, 0xe5),
        (0x7f, 0x7f, 0x7f),
        (0xff, 0x4b, 0x4b),
        (0x00, 0xff, 0x00),
        (0xff, 0xff, 0x00),
        (0x5c, 0x8c, 0xff),
        (0xff, 0x00, 0xff),
        (0x00, 0xff, 0xff),
        (0xff, 0xff, 0xff),
    ];
    match i {
        0..=15 => {
            let (r, g, b) = ANSI[i as usize];
            Color32::from_rgb(r, g, b)
        }
        16..=231 => {
            let i = i - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + 40 * v };
            Color32::from_rgb(level(i / 36), level((i / 6) % 6), level(i % 6))
        }
        _ => {
            let g = 8 + 10 * (i - 232);
            Color32::from_rgb(g, g, g)
        }
    }
}

/// Bytes a terminal sends for a non-text key press, or None if the key
/// produces a regular `Event::Text` (or nothing).
fn encode_key(key: Key, mods: Modifiers) -> Option<Vec<u8>> {
    let seq: &[u8] = match key {
        Key::Enter => b"\r",
        Key::Tab => b"\t",
        Key::Backspace => b"\x7f",
        Key::Escape => b"\x1b",
        Key::ArrowUp => b"\x1b[A",
        Key::ArrowDown => b"\x1b[B",
        Key::ArrowRight => b"\x1b[C",
        Key::ArrowLeft => b"\x1b[D",
        Key::Home => b"\x1b[H",
        Key::End => b"\x1b[F",
        Key::Insert => b"\x1b[2~",
        Key::Delete => b"\x1b[3~",
        Key::PageUp => b"\x1b[5~",
        Key::PageDown => b"\x1b[6~",
        _ => {
            // Ctrl+letter -> C0 control byte (Ctrl+C = 0x03, ...).
            if mods.ctrl && !mods.alt {
                let name = key.name();
                if let [c @ b'A'..=b'Z'] = name.as_bytes() {
                    return Some(vec![c - b'A' + 1]);
                }
            }
            return None;
        }
    };
    Some(seq.to_vec())
}

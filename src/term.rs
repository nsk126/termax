//! The terminal screen model: a grid of styled cells plus a cursor,
//! updated by interpreting the escape-sequence stream from the child
//! process via the `vte` parser.
//!
//! This is the renderer-agnostic core. A GUI front end (wgpu, egui, ...)
//! only needs to read `Grid::cells` and `Grid::cursor` each frame.

use vte::{Params, Perform};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Color {
    #[default]
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Style {
    pub fg: Color,
    pub bg: Color,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Cell {
    pub ch: char,
    pub style: Style,
}

impl Default for Cell {
    fn default() -> Self {
        Cell { ch: ' ', style: Style::default() }
    }
}

pub struct Grid {
    pub rows: usize,
    pub cols: usize,
    pub cells: Vec<Vec<Cell>>,
    /// (row, col), 0-based.
    pub cursor: (usize, usize),
    /// Attributes applied to newly printed cells (set by SGR).
    pen: Style,
    /// Set when the alternate screen is active (e.g. inside vim/less).
    pub alt_screen: bool,
}

impl Grid {
    pub fn new(rows: usize, cols: usize) -> Self {
        Grid {
            rows,
            cols,
            cells: vec![vec![Cell::default(); cols]; rows],
            cursor: (0, 0),
            pen: Style::default(),
            alt_screen: false,
        }
    }

    fn blank(&self) -> Cell {
        Cell { ch: ' ', style: Style { bg: self.pen.bg, ..Style::default() } }
    }

    fn scroll_up(&mut self) {
        self.cells.remove(0);
        let blank_row = vec![self.blank(); self.cols];
        self.cells.push(blank_row);
    }

    fn linefeed(&mut self) {
        if self.cursor.0 + 1 >= self.rows {
            self.scroll_up();
        } else {
            self.cursor.0 += 1;
        }
    }

    fn put_char(&mut self, ch: char) {
        if self.cursor.1 >= self.cols {
            // Deferred wrap: printing past the last column moves to the next line.
            self.cursor.1 = 0;
            self.linefeed();
        }
        let (r, c) = self.cursor;
        self.cells[r][c] = Cell { ch, style: self.pen };
        self.cursor.1 += 1;
    }

    fn erase_in_display(&mut self, mode: u16) {
        let (r, c) = self.cursor;
        let blank = self.blank();
        match mode {
            0 => {
                for col in c..self.cols {
                    self.cells[r][col] = blank;
                }
                for row in (r + 1)..self.rows {
                    self.cells[row].fill(blank);
                }
            }
            1 => {
                for row in 0..r {
                    self.cells[row].fill(blank);
                }
                for col in 0..=c.min(self.cols - 1) {
                    self.cells[r][col] = blank;
                }
            }
            _ => {
                for row in &mut self.cells {
                    row.fill(blank);
                }
            }
        }
    }

    fn erase_in_line(&mut self, mode: u16) {
        let (r, c) = self.cursor;
        let blank = self.blank();
        match mode {
            0 => self.cells[r][c.min(self.cols - 1)..].fill(blank),
            1 => self.cells[r][..=c.min(self.cols - 1)].fill(blank),
            _ => self.cells[r].fill(blank),
        }
    }

    fn sgr(&mut self, params: &Params) {
        let mut iter = params.iter();
        while let Some(param) = iter.next() {
            match param[0] {
                0 => self.pen = Style::default(),
                1 => self.pen.bold = true,
                3 => self.pen.italic = true,
                4 => self.pen.underline = true,
                7 => self.pen.inverse = true,
                22 => self.pen.bold = false,
                23 => self.pen.italic = false,
                24 => self.pen.underline = false,
                27 => self.pen.inverse = false,
                30..=37 => self.pen.fg = Color::Indexed(param[0] as u8 - 30),
                39 => self.pen.fg = Color::Default,
                40..=47 => self.pen.bg = Color::Indexed(param[0] as u8 - 40),
                49 => self.pen.bg = Color::Default,
                90..=97 => self.pen.fg = Color::Indexed(param[0] as u8 - 90 + 8),
                100..=107 => self.pen.bg = Color::Indexed(param[0] as u8 - 100 + 8),
                38 | 48 => {
                    // Extended color: 38;5;n (256-color) or 38;2;r;g;b (truecolor).
                    // Subparams may arrive colon-separated (in `param`) or
                    // semicolon-separated (as following params).
                    let is_fg = param[0] == 38;
                    let color = if param.len() >= 3 && param[1] == 5 {
                        Some(Color::Indexed(param[2] as u8))
                    } else if param.len() >= 5 && param[1] == 2 {
                        Some(Color::Rgb(param[2] as u8, param[3] as u8, param[4] as u8))
                    } else if param.len() == 1 {
                        match iter.next().map(|p| p[0]) {
                            Some(5) => iter.next().map(|p| Color::Indexed(p[0] as u8)),
                            Some(2) => {
                                let (r, g, b) = (iter.next(), iter.next(), iter.next());
                                match (r, g, b) {
                                    (Some(r), Some(g), Some(b)) => {
                                        Some(Color::Rgb(r[0] as u8, g[0] as u8, b[0] as u8))
                                    }
                                    _ => None,
                                }
                            }
                            _ => None,
                        }
                    } else {
                        None
                    };
                    if let Some(color) = color {
                        if is_fg {
                            self.pen.fg = color;
                        } else {
                            self.pen.bg = color;
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// Plain-text dump of the screen contents, trailing blanks trimmed.
    /// Useful for debugging and for an agent to "read" the screen.
    pub fn to_text(&self) -> String {
        let mut lines: Vec<String> = self
            .cells
            .iter()
            .map(|row| {
                let line: String = row.iter().map(|c| c.ch).collect();
                line.trim_end().to_string()
            })
            .collect();
        while lines.last().is_some_and(|l| l.is_empty()) {
            lines.pop();
        }
        lines.join("\n")
    }
}

impl Perform for Grid {
    fn print(&mut self, ch: char) {
        self.put_char(ch);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' | 0x0b | 0x0c => self.linefeed(),
            b'\r' => self.cursor.1 = 0,
            0x08 => self.cursor.1 = self.cursor.1.saturating_sub(1),
            b'\t' => {
                let next_stop = (self.cursor.1 / 8 + 1) * 8;
                self.cursor.1 = next_stop.min(self.cols - 1);
            }
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], _ignore: bool, action: char) {
        let mut args = params.iter().map(|p| p[0]);
        let arg = |v: Option<u16>, default: u16| v.filter(|&x| x != 0).unwrap_or(default);
        match action {
            'H' | 'f' => {
                let row = arg(args.next(), 1) as usize;
                let col = arg(args.next(), 1) as usize;
                self.cursor = (
                    (row - 1).min(self.rows - 1),
                    (col - 1).min(self.cols - 1),
                );
            }
            'A' => self.cursor.0 = self.cursor.0.saturating_sub(arg(args.next(), 1) as usize),
            'B' => self.cursor.0 = (self.cursor.0 + arg(args.next(), 1) as usize).min(self.rows - 1),
            'C' => self.cursor.1 = (self.cursor.1 + arg(args.next(), 1) as usize).min(self.cols - 1),
            'D' => self.cursor.1 = self.cursor.1.saturating_sub(arg(args.next(), 1) as usize),
            'G' => self.cursor.1 = (arg(args.next(), 1) as usize - 1).min(self.cols - 1),
            'd' => self.cursor.0 = (arg(args.next(), 1) as usize - 1).min(self.rows - 1),
            'J' => self.erase_in_display(args.next().unwrap_or(0)),
            'K' => self.erase_in_line(args.next().unwrap_or(0)),
            'm' => self.sgr(params),
            'h' | 'l' if intermediates == b"?" => {
                // DEC private modes; track the alternate screen (1049 / 47).
                if let Some(mode) = args.next() {
                    if mode == 1049 || mode == 47 {
                        self.alt_screen = action == 'h';
                        if action == 'h' {
                            self.erase_in_display(2);
                            self.cursor = (0, 0);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn esc_dispatch(&mut self, _intermediates: &[u8], _ignore: bool, byte: u8) {
        if byte == b'c' {
            // RIS: full reset.
            let blank = Cell::default();
            for row in &mut self.cells {
                row.fill(blank);
            }
            self.cursor = (0, 0);
            self.pen = Style::default();
        }
    }

    fn hook(&mut self, _: &Params, _: &[u8], _: bool, _: char) {}
    fn put(&mut self, _: u8) {}
    fn unhook(&mut self) {}
    fn osc_dispatch(&mut self, _: &[&[u8]], _: bool) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use vte::Parser;

    fn feed(grid: &mut Grid, bytes: &[u8]) {
        let mut parser = Parser::new();
        for &b in bytes {
            parser.advance(grid, b);
        }
    }

    #[test]
    fn prints_and_wraps() {
        let mut grid = Grid::new(3, 5);
        feed(&mut grid, b"hello world");
        assert_eq!(grid.to_text(), "hello\n worl\nd");
    }

    #[test]
    fn crlf_and_overwrite() {
        let mut grid = Grid::new(3, 10);
        feed(&mut grid, b"abc\r\ndef\rD");
        assert_eq!(grid.to_text(), "abc\nDef");
    }

    #[test]
    fn cursor_positioning() {
        let mut grid = Grid::new(5, 10);
        feed(&mut grid, b"\x1b[3;4Hx");
        assert_eq!(grid.cells[2][3].ch, 'x');
    }

    #[test]
    fn sgr_colors() {
        let mut grid = Grid::new(2, 10);
        feed(&mut grid, b"\x1b[1;31mR\x1b[0mn\x1b[38;5;200mP");
        assert_eq!(grid.cells[0][0].style.fg, Color::Indexed(1));
        assert!(grid.cells[0][0].style.bold);
        assert_eq!(grid.cells[0][1].style, Style::default());
        assert_eq!(grid.cells[0][2].style.fg, Color::Indexed(200));
    }

    #[test]
    fn clear_screen() {
        let mut grid = Grid::new(3, 10);
        feed(&mut grid, b"junk\x1b[2J\x1b[Hok");
        assert_eq!(grid.to_text(), "ok");
    }

    #[test]
    fn scrolls_when_past_bottom() {
        let mut grid = Grid::new(2, 10);
        feed(&mut grid, b"one\r\ntwo\r\nthree");
        assert_eq!(grid.to_text(), "two\nthree");
    }
}

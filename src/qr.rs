//! An invitation as a QR code drawn in the terminal, so a phone can scan it
//! off the screen.
//!
//! Each character cell holds two rows of modules, using the upper and lower
//! half block characters. Light modules are drawn and dark ones are left as
//! the terminal background, so the code reads the right way round on a dark
//! terminal. A quiet zone of light modules surrounds it.

use anyhow::Result;
use qrcode::{Color, QrCode};

/// Light modules around the code. The spec asks for 4.
const QUIET: usize = 4;

const FULL: char = '\u{2588}';
const UPPER: char = '\u{2580}';
const LOWER: char = '\u{2584}';

/// `text` as a QR code, one line per two rows of modules.
pub fn render(text: &str) -> Result<String> {
    let code = QrCode::new(text.as_bytes())?;
    let width = code.width();
    let colors = code.to_colors();
    let side = width + 2 * QUIET;
    let light = |x: usize, y: usize| -> bool {
        if x < QUIET || y < QUIET || x >= width + QUIET || y >= width + QUIET {
            return true;
        }
        colors[(y - QUIET) * width + (x - QUIET)] == Color::Light
    };

    let mut out = String::new();
    for y in (0..side).step_by(2) {
        for x in 0..side {
            let top = light(x, y);
            let bottom = y + 1 < side && light(x, y + 1);
            out.push(match (top, bottom) {
                (true, true) => FULL,
                (true, false) => UPPER,
                (false, true) => LOWER,
                (false, false) => ' ',
            });
        }
        out.push('\n');
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Read the drawing back into pixels (dark on light, as a camera on a
    /// dark terminal sees it) and decode it.
    fn decode(drawing: &str) -> String {
        let rows: Vec<Vec<char>> = drawing.lines().map(|l| l.chars().collect()).collect();
        let side = rows[0].len();
        let mut light = vec![vec![false; side]; rows.len() * 2];
        for (r, row) in rows.iter().enumerate() {
            assert_eq!(row.len(), side, "every line is as wide as the first");
            for (x, c) in row.iter().enumerate() {
                let (top, bottom) = match *c {
                    FULL => (true, true),
                    UPPER => (true, false),
                    LOWER => (false, true),
                    ' ' => (false, false),
                    other => panic!("unexpected {:?}", other),
                };
                light[2 * r][x] = top;
                light[2 * r + 1][x] = bottom;
            }
        }
        let scale = 4;
        let (w, h) = (side * scale, light.len() * scale);
        let mut img = rqrr::PreparedImage::prepare_from_greyscale(w, h, |x, y| {
            if light[y / scale][x / scale] {
                255
            } else {
                0
            }
        });
        let grids = img.detect_grids();
        assert_eq!(grids.len(), 1, "one code in the drawing");
        grids[0].decode().unwrap().1
    }

    #[test]
    fn the_drawing_decodes_to_the_invitation() {
        let invite = "5hc4bjqfp6d3k2m8n9q7r1s0t4v6w8x2y3z5a7b9c1d3e5f7g9.\
                      7fd25613dd0a4b8c9e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2c3d";
        let drawing = render(invite).unwrap();
        assert_eq!(decode(&drawing), invite);
    }

    #[test]
    fn the_quiet_zone_is_drawn() {
        let drawing = render("abc").unwrap();
        let first = drawing.lines().next().unwrap();
        assert!(first.chars().all(|c| c == FULL));
        let width = QrCode::new(b"abc").unwrap().width();
        assert_eq!(first.chars().count(), width + 2 * QUIET);
    }
}

#[allow(dead_code)]
pub struct Theme {
    pub bg: [u8; 4],
    pub fg: [u8; 3],
    pub cursor: [u8; 4],
    pub selection_bg: [f32; 4],
    pub overlay_cursor_color: [f32; 4],
    pub palette: [[u8; 3]; 256],
}

impl Default for Theme {
    fn default() -> Self {
        let mut palette = xterm_256_palette();

        // Override first 16 with Catppuccin Frappe ANSI colors.
        let frappe_ansi: [[u8; 3]; 16] = [
            [81, 87, 109],   // 0  black    (surface1)
            [231, 130, 132], // 1  red
            [166, 209, 137], // 2  green
            [229, 200, 144], // 3  yellow
            [140, 170, 238], // 4  blue
            [244, 184, 228], // 5  magenta
            [129, 200, 190], // 6  cyan
            [181, 191, 226], // 7  white    (subtext1)
            [98, 104, 128],  // 8  bright black  (overlay0)
            [231, 130, 132], // 9  bright red
            [166, 209, 137], // 10 bright green
            [229, 200, 144], // 11 bright yellow
            [140, 170, 238], // 12 bright blue
            [244, 184, 228], // 13 bright magenta
            [129, 200, 190], // 14 bright cyan
            [198, 208, 245], // 15 bright white  (text)
        ];
        palette[..16].copy_from_slice(&frappe_ansi);

        Self {
            bg: [48, 52, 70, 255],
            fg: [198, 208, 245],
            cursor: [242, 213, 207, 255],
            selection_bg: [0.3, 0.5, 0.8, 0.4],
            overlay_cursor_color: [1.0, 1.0, 1.0, 1.0],
            palette,
        }
    }
}

impl Theme {
    /// Resolve a VT-reported color into concrete RGB, or `None` when
    /// the cell wants the theme default.
    pub fn resolve_color(&self, color: &seance_vt::CellColor) -> Option<[u8; 3]> {
        use seance_vt::CellColor;
        match *color {
            CellColor::Default => None,
            CellColor::Palette(idx) => Some(self.palette[idx as usize]),
            CellColor::Rgb(r, g, b) => Some([r, g, b]),
        }
    }
}

fn xterm_256_palette() -> [[u8; 3]; 256] {
    let mut p = [[0u8; 3]; 256];

    // 0-15: defaults (overridden by theme)
    let base16: [[u8; 3]; 16] = [
        [0, 0, 0],
        [128, 0, 0],
        [0, 128, 0],
        [128, 128, 0],
        [0, 0, 128],
        [128, 0, 128],
        [0, 128, 128],
        [192, 192, 192],
        [128, 128, 128],
        [255, 0, 0],
        [0, 255, 0],
        [255, 255, 0],
        [0, 0, 255],
        [255, 0, 255],
        [0, 255, 255],
        [255, 255, 255],
    ];
    p[..16].copy_from_slice(&base16);

    // 16-231: 6x6x6 color cube
    for r in 0..6u8 {
        for g in 0..6u8 {
            for b in 0..6u8 {
                let idx = 16 + (r as usize) * 36 + (g as usize) * 6 + (b as usize);
                p[idx] = [
                    if r == 0 { 0 } else { 55 + 40 * r },
                    if g == 0 { 0 } else { 55 + 40 * g },
                    if b == 0 { 0 } else { 55 + 40 * b },
                ];
            }
        }
    }

    // 232-255: grayscale ramp
    for i in 0..24u8 {
        let v = 8 + 10 * i;
        p[232 + i as usize] = [v, v, v];
    }

    p
}

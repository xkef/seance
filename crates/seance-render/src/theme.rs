pub struct Theme {
    pub bg: [u8; 4],
    pub fg: [u8; 3],
    pub cursor: [u8; 4],
    pub selection_bg: [f32; 4],
    pub palette: [[u8; 3]; 256],
}

impl Default for Theme {
    fn default() -> Self {
        let mut palette = [[0u8; 3]; 256];
        palette[..16].copy_from_slice(&CATPPUCCIN_FRAPPE_ANSI);
        fill_xterm_cube(&mut palette);
        fill_xterm_grayscale(&mut palette);

        Self {
            bg: [48, 52, 70, 255],
            fg: [198, 208, 245],
            cursor: [242, 213, 207, 255],
            selection_bg: [0.3, 0.5, 0.8, 0.4],
            palette,
        }
    }
}

impl Theme {
    /// Resolve a VT-reported color into RGB, or `None` for the theme default.
    pub fn resolve_color(&self, color: &seance_vt::CellColor) -> Option<[u8; 3]> {
        use seance_vt::CellColor;
        match *color {
            CellColor::Default => None,
            CellColor::Palette(idx) => Some(self.palette[idx as usize]),
            CellColor::Rgb(r, g, b) => Some([r, g, b]),
        }
    }
}

/// ANSI 0-15 colors (Catppuccin Frappe).
const CATPPUCCIN_FRAPPE_ANSI: [[u8; 3]; 16] = [
    [81, 87, 109],   //  0 black    (surface1)
    [231, 130, 132], //  1 red
    [166, 209, 137], //  2 green
    [229, 200, 144], //  3 yellow
    [140, 170, 238], //  4 blue
    [244, 184, 228], //  5 magenta
    [129, 200, 190], //  6 cyan
    [181, 191, 226], //  7 white    (subtext1)
    [98, 104, 128],  //  8 bright black  (overlay0)
    [231, 130, 132], //  9 bright red
    [166, 209, 137], // 10 bright green
    [229, 200, 144], // 11 bright yellow
    [140, 170, 238], // 12 bright blue
    [244, 184, 228], // 13 bright magenta
    [129, 200, 190], // 14 bright cyan
    [198, 208, 245], // 15 bright white  (text)
];

/// xterm 6×6×6 color cube in slots 16..232.
fn fill_xterm_cube(p: &mut [[u8; 3]; 256]) {
    let step = |v: u8| if v == 0 { 0 } else { 55 + 40 * v };
    for r in 0..6u8 {
        for g in 0..6u8 {
            for b in 0..6u8 {
                let idx = 16 + (r as usize) * 36 + (g as usize) * 6 + (b as usize);
                p[idx] = [step(r), step(g), step(b)];
            }
        }
    }
}

/// xterm grayscale ramp in slots 232..256.
fn fill_xterm_grayscale(p: &mut [[u8; 3]; 256]) {
    for i in 0..24u8 {
        let v = 8 + 10 * i;
        p[232 + i as usize] = [v, v, v];
    }
}

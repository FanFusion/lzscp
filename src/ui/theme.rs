use ratatui::style::Color;

#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub bg: Color,
    pub fg: Color,
    pub accent: Color,
    pub muted: Color,
    pub selection: Color,
    pub diff_add: Color,
    pub diff_del: Color,
}

pub fn palette(name: &str) -> Palette {
    match name {
        "tokyo_night" | "tokyonight" => Palette {
            bg: Color::Rgb(26, 27, 38),
            fg: Color::Rgb(192, 202, 245),
            accent: Color::Rgb(122, 162, 247),
            muted: Color::Rgb(86, 95, 137),
            selection: Color::Rgb(40, 52, 87),
            diff_add: Color::Rgb(158, 206, 106),
            diff_del: Color::Rgb(247, 118, 142),
        },
        "gruvbox" => Palette {
            bg: Color::Rgb(29, 32, 33),
            fg: Color::Rgb(235, 219, 178),
            accent: Color::Rgb(250, 189, 47),
            muted: Color::Rgb(146, 131, 116),
            selection: Color::Rgb(60, 56, 54),
            diff_add: Color::Rgb(184, 187, 38),
            diff_del: Color::Rgb(251, 73, 52),
        },
        "nord" => Palette {
            bg: Color::Rgb(46, 52, 64),
            fg: Color::Rgb(216, 222, 233),
            accent: Color::Rgb(136, 192, 208),
            muted: Color::Rgb(110, 122, 142),
            selection: Color::Rgb(67, 76, 94),
            diff_add: Color::Rgb(163, 190, 140),
            diff_del: Color::Rgb(191, 97, 106),
        },
        "dracula" => Palette {
            bg: Color::Rgb(40, 42, 54),
            fg: Color::Rgb(248, 248, 242),
            accent: Color::Rgb(189, 147, 249),
            muted: Color::Rgb(98, 114, 164),
            selection: Color::Rgb(68, 71, 90),
            diff_add: Color::Rgb(80, 250, 123),
            diff_del: Color::Rgb(255, 85, 85),
        },
        _ => Palette {
            // mocha (catppuccin-mocha) — default
            bg: Color::Rgb(30, 30, 46),
            fg: Color::Rgb(205, 214, 244),
            accent: Color::Rgb(203, 166, 247),
            muted: Color::Rgb(108, 112, 134),
            selection: Color::Rgb(69, 71, 90),
            diff_add: Color::Rgb(166, 227, 161),
            diff_del: Color::Rgb(243, 139, 168),
        },
    }
}

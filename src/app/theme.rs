use crate::settings::Theme;
use ratatui::style::{Color, Style};

pub(super) struct Palette {
    pub bg: Color,
    pub fg: Color,
    pub dim: Color,
    pub accent: Color,
    pub select: Color,
    pub warn: Color,
    pub added: Color,
    pub removed: Color,
}

impl Theme {
    pub(super) fn palette(self) -> Palette {
        match self {
            Self::Dark => Palette {
                bg: Color::Rgb(22, 27, 34),
                fg: Color::Rgb(207, 216, 227),
                dim: Color::Rgb(126, 141, 160),
                accent: Color::Rgb(151, 212, 185),
                select: Color::Rgb(48, 68, 86),
                warn: Color::Rgb(229, 192, 123),
                added: Color::Rgb(152, 195, 121),
                removed: Color::Rgb(224, 108, 117),
            },
            Self::Light => Palette {
                bg: Color::Rgb(247, 248, 250),
                fg: Color::Rgb(36, 45, 57),
                dim: Color::Rgb(91, 105, 121),
                accent: Color::Rgb(24, 108, 81),
                select: Color::Rgb(210, 228, 241),
                warn: Color::Rgb(145, 88, 12),
                added: Color::Rgb(38, 112, 51),
                removed: Color::Rgb(173, 47, 65),
            },
        }
    }

    /// The source layout's semantic colours are stored in the dark palette.
    /// Map at paint time so changing theme never touches text or layout caches.
    pub(super) fn text_style(self, mut style: Style) -> Style {
        if self == Self::Light {
            style.fg = style.fg.map(|colour| match colour {
                Color::Rgb(151, 212, 185) => self.palette().accent,
                Color::Rgb(242, 239, 226) => Color::Rgb(15, 25, 38),
                Color::Rgb(225, 191, 136) => Color::Rgb(133, 75, 21),
                Color::Rgb(133, 187, 233) => Color::Rgb(32, 95, 165),
                Color::Rgb(139, 154, 177) => self.palette().dim,
                Color::Rgb(77, 96, 111) => Color::Rgb(131, 145, 157),
                other => other,
            });
        }
        style
    }
}

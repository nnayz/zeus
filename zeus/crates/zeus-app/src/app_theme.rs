use gpui::Rgba;
use zeus_term::theme::{TermTheme, ThemeAppearance};
use zeus_ui::{Appearance, SemanticColors};

/// Resolves persisted theme ids in one place for both terminal and app chrome.
pub(crate) fn terminal_theme(id: &str) -> TermTheme {
    TermTheme::CATALOG
        .into_iter()
        .find(|theme| theme.id == id)
        .unwrap_or_default()
}

pub(crate) fn colors(id: &str) -> SemanticColors {
    semantic_colors(terminal_theme(id), false)
}

pub(crate) fn sidebar_colors(id: &str) -> SemanticColors {
    semantic_colors(terminal_theme(id), true)
}

fn semantic_colors(theme: TermTheme, sidebar_tones: bool) -> SemanticColors {
    // The Zeus themes use deliberate neutral surface steps: ChatGPT-like
    // charcoal for the default and a near-black hierarchy for high contrast.
    // Other catalog themes retain derived surfaces so their tint carries
    // through the application.
    let (sidebar_surface, floating_surface) = match theme.id {
        id if id == TermTheme::ZEUS_DARK.id => (hex(0x171717), hex(0x2f2f2f)),
        id if id == TermTheme::ZEUS_DARK_HIGH_CONTRAST.id => (hex(0x0a0a0a), hex(0x1a1a1a)),
        _ => (
            mix(theme.background, theme.foreground, 0.08, 0.92),
            mix(theme.background, theme.foreground, 0.13, 1.0),
        ),
    };
    let mut colors = SemanticColors::themed(
        match theme.appearance {
            ThemeAppearance::Dark => Appearance::Dark,
            ThemeAppearance::Light => Appearance::Light,
        },
        theme.background,
        theme.foreground,
        sidebar_surface,
        floating_surface,
        sidebar_tones,
    );
    if theme.id == TermTheme::ZEUS_DARK.id {
        colors.secondary = hex(0xb4b4b4);
        colors.tertiary = hex(0x8e8e8e);
    } else if theme.id == TermTheme::ZEUS_DARK_HIGH_CONTRAST.id {
        colors.secondary = hex(0xd4d4d4);
        colors.tertiary = hex(0xa3a3a3);
    }
    colors
}

const fn hex(value: u32) -> Rgba {
    Rgba {
        r: ((value >> 16) & 0xff) as f32 / 255.0,
        g: ((value >> 8) & 0xff) as f32 / 255.0,
        b: (value & 0xff) as f32 / 255.0,
        a: 1.0,
    }
}

fn mix(background: Rgba, foreground: Rgba, amount: f32, alpha: f32) -> Rgba {
    let inverse = 1.0 - amount;
    Rgba {
        r: background.r * inverse + foreground.r * amount,
        g: background.g * inverse + foreground.g * amount,
        b: background.b * inverse + foreground.b * amount,
        a: alpha,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_terminal_theme_drives_application_semantics() {
        let dracula = terminal_theme("dracula");
        let dracula_app = colors("dracula");
        let solarized_app = colors("solarized-dark");

        assert_eq!(dracula_app.background, dracula.background);
        assert_eq!(dracula_app.primary, dracula.foreground);
        assert_ne!(dracula_app.background, solarized_app.background);
        assert_ne!(
            dracula_app.sidebar_surface(),
            solarized_app.sidebar_surface()
        );
        assert_ne!(
            dracula_app.floating_surface(),
            solarized_app.floating_surface()
        );
    }

    #[test]
    fn sidebar_palette_keeps_stronger_supporting_text() {
        let base = colors("tokyo-night");
        let sidebar = sidebar_colors("tokyo-night");
        assert!(sidebar.secondary.a > base.secondary.a);
        assert!(sidebar.tertiary.a > base.tertiary.a);
    }

    #[test]
    fn zeus_dark_uses_neutral_chatgpt_style_surfaces_and_text() {
        let app = colors(TermTheme::ZEUS_DARK.id);

        assert_eq!(app.background, hex(0x212121));
        assert_eq!(app.primary, hex(0xececec));
        assert_eq!(app.secondary, hex(0xb4b4b4));
        assert_eq!(app.tertiary, hex(0x8e8e8e));
        assert_eq!(app.sidebar_surface(), hex(0x171717));
        assert_eq!(app.floating_surface(), hex(0x2f2f2f));
    }

    #[test]
    fn zeus_dark_high_contrast_uses_brighter_text_and_black_surfaces() {
        let standard = colors(TermTheme::ZEUS_DARK.id);
        let high_contrast = colors(TermTheme::ZEUS_DARK_HIGH_CONTRAST.id);

        assert_eq!(high_contrast.background, hex(0x000000));
        assert_eq!(high_contrast.primary, hex(0xffffff));
        assert_eq!(high_contrast.secondary, hex(0xd4d4d4));
        assert_eq!(high_contrast.tertiary, hex(0xa3a3a3));
        assert_eq!(high_contrast.sidebar_surface(), hex(0x0a0a0a));
        assert_eq!(high_contrast.floating_surface(), hex(0x1a1a1a));
        assert!(high_contrast.primary.r > standard.primary.r);
        assert!(high_contrast.secondary.r > standard.secondary.r);
        assert!(high_contrast.background.r < standard.background.r);
    }

    #[test]
    fn light_terminal_themes_produce_light_application_semantics() {
        let theme = terminal_theme("zeus-light");
        let app = colors(theme.id);

        assert_eq!(app.appearance, Appearance::Light);
        assert_eq!(app.background, theme.background);
        assert_eq!(app.primary, theme.foreground);
        assert_eq!(app.floating_stroke().r, 0.0);
        assert_eq!(app.floating_stroke().a, 0.10);
    }
}

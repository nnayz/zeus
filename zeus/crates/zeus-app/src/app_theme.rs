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
    // Zeus Dark follows an IDE-style three-surface hierarchy: subdued chrome,
    // an editor canvas, then slightly lifted transient UI. Other catalog
    // themes retain their derived surfaces so their individual tint carries
    // through the application.
    let (sidebar_surface, floating_surface) = if theme.id == TermTheme::ZEUS_DARK.id {
        (hex(0x181818), hex(0x222222))
    } else {
        (
            mix(theme.background, theme.foreground, 0.08, 0.92),
            mix(theme.background, theme.foreground, 0.13, 1.0),
        )
    };
    SemanticColors::themed(
        match theme.appearance {
            ThemeAppearance::Dark => Appearance::Dark,
            ThemeAppearance::Light => Appearance::Light,
        },
        theme.background,
        theme.foreground,
        sidebar_surface,
        floating_surface,
        sidebar_tones,
    )
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
    fn zeus_dark_uses_cursor_style_workbench_surfaces() {
        let app = colors(TermTheme::ZEUS_DARK.id);

        assert_eq!(app.background, hex(0x1f1f1f));
        assert_eq!(app.primary, hex(0xcccccc));
        assert_eq!(app.sidebar_surface(), hex(0x181818));
        assert_eq!(app.floating_surface(), hex(0x222222));
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

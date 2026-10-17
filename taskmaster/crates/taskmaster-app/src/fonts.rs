//! Runtime font-family selection.
//!
//! `.SystemUIFont` is GPUI's virtual family for the platform UI font. On macOS
//! it resolves to CoreText's `.AppleSystemUIFont`, preserving native optical
//! sizing without copying font files into GPUI's in-memory font source. SF Mono
//! is optional, so terminal text falls back to the always-installed Menlo.

use std::collections::HashSet;
use std::sync::OnceLock;

use gpui::App;

static UI_FAMILY: OnceLock<&'static str> = OnceLock::new();
static MONO_FAMILY: OnceLock<&'static str> = OnceLock::new();

/// Call once at startup, after GPUI has discovered the system font catalog.
pub fn init(cx: &App) {
    let names: HashSet<String> = cx.text_system().all_font_names().into_iter().collect();
    let _ = UI_FAMILY.set(".SystemUIFont");
    let _ = MONO_FAMILY.set(select_mono(&names));
}

pub fn ui_family() -> &'static str {
    UI_FAMILY.get().copied().unwrap_or(".SystemUIFont")
}

pub fn mono_family() -> &'static str {
    MONO_FAMILY.get().copied().unwrap_or("Menlo")
}

fn select_mono(names: &HashSet<String>) -> &'static str {
    if names.contains("SF Mono") {
        "SF Mono"
    } else {
        "Menlo"
    }
}

#[cfg(test)]
mod tests {
    use super::select_mono;
    use std::collections::HashSet;

    #[test]
    fn mono_selection_uses_discovered_sf_mono_or_safe_system_fallback() {
        assert_eq!(select_mono(&HashSet::new()), "Menlo");
        assert_eq!(
            select_mono(&HashSet::from(["SF Mono".to_owned()])),
            "SF Mono"
        );
    }
}

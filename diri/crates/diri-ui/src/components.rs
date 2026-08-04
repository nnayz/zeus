use std::time::Duration;

use gpui::{
    Animation, AnimationExt, AnyElement, App, BoxShadow, IntoElement, RenderOnce, Rgba, Window,
    div, ease_out_quint, point, prelude::*, px,
};

use crate::{Fill, Radius, SemanticColors, rgba_f32};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RowFill {
    #[default]
    Clear,
    Hover,
    MultiSelected,
    Selected,
}

impl RowFill {
    pub fn color(self, colors: SemanticColors) -> Rgba {
        match self {
            Self::Clear => colors.primary.alpha(0.0),
            Self::Hover => colors.primary.alpha(Fill::HOVER_OPACITY),
            Self::MultiSelected => colors.primary.alpha(Fill::MULTI_SELECTED_OPACITY),
            Self::Selected => colors.primary.alpha(Fill::SELECTED_OPACITY),
        }
    }
}

/// Shared panel recipe for palettes, popovers, and find surfaces.
#[derive(IntoElement)]
pub struct FloatingSurface {
    colors: SemanticColors,
    child: AnyElement,
}

impl FloatingSurface {
    pub fn new(colors: SemanticColors, child: impl IntoElement) -> Self {
        Self {
            colors,
            child: child.into_any_element(),
        }
    }
}

impl RenderOnce for FloatingSurface {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let colors = self.colors;
        div()
            .relative()
            .rounded(px(Radius::PANEL))
            // Floating chrome keeps the sidebar hue but uses a denser material
            // so live terminal content never competes with labels or controls.
            .bg(colors.floating_surface())
            .border_1()
            .border_color(colors.floating_stroke())
            .shadow(vec![
                BoxShadow {
                    color: rgba_f32(0.0, 0.0, 0.0, 0.32).into(),
                    offset: point(px(0.0), px(14.0)),
                    blur_radius: px(32.0),
                    spread_radius: px(0.0),
                    inset: false,
                },
                BoxShadow {
                    color: colors.primary.alpha(0.035).into(),
                    offset: point(px(0.0), px(1.0)),
                    blur_radius: px(0.0),
                    spread_radius: px(0.0),
                    inset: true,
                },
            ])
            .child(self.child)
            .with_animation(
                "floating-surface-entry",
                Animation::new(Duration::from_millis(160)).with_easing(ease_out_quint()),
                |surface, delta| surface.opacity(0.76 + 0.24 * delta),
            )
    }
}

/// A one-point divider using the foreground color at six percent opacity.
#[derive(IntoElement)]
pub struct HairlineDivider {
    colors: SemanticColors,
    vertical: bool,
}

impl HairlineDivider {
    pub const fn horizontal(colors: SemanticColors) -> Self {
        Self {
            colors,
            vertical: false,
        }
    }

    pub const fn vertical(colors: SemanticColors) -> Self {
        Self {
            colors,
            vertical: true,
        }
    }
}

impl RenderOnce for HairlineDivider {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        div()
            .flex_none()
            .bg(self.colors.primary.alpha(0.06))
            .when(self.vertical, |element| element.w(px(1.0)).h_full())
            .when(!self.vertical, |element| element.h(px(1.0)).w_full())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_fill_scale_is_shared() {
        let colors = SemanticColors::dark();
        assert_eq!(RowFill::Hover.color(colors).a, 0.06);
        assert_eq!(RowFill::MultiSelected.color(colors).a, 0.08);
        assert_eq!(RowFill::Selected.color(colors).a, 0.10);
        assert_eq!(RowFill::Clear.color(colors).a, 0.0);
    }
}

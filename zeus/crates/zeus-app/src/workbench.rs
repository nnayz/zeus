//! Pure workbench layout state.
//!
//! GPUI owns pixels and pointer routing; this module owns only the durable
//! split intent. Keeping that seam free of views makes it straightforward to
//! add more pane kinds and a trailing inspector without teaching terminal
//! rendering about workbench policy.

pub const DEFAULT_PRIMARY_FRACTION: f32 = 0.62;
const MIN_PRIMARY_HEIGHT: f32 = 220.0;
const MIN_AUXILIARY_HEIGHT: f32 = 140.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaneHeights {
    pub primary: f32,
    pub auxiliary: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorkbenchLayout {
    primary_fraction: f32,
}

impl Default for WorkbenchLayout {
    fn default() -> Self {
        Self {
            primary_fraction: DEFAULT_PRIMARY_FRACTION,
        }
    }
}

impl WorkbenchLayout {
    pub fn from_fraction(primary_fraction: f32) -> Self {
        let mut layout = Self::default();
        if primary_fraction.is_finite() {
            layout.primary_fraction = primary_fraction.clamp(0.0, 1.0);
        }
        layout
    }

    pub fn primary_fraction(&self) -> f32 {
        self.primary_fraction
    }

    pub fn pane_heights(&self, available_height: f32) -> PaneHeights {
        let available_height = available_height.max(0.0);
        if available_height <= MIN_PRIMARY_HEIGHT + MIN_AUXILIARY_HEIGHT {
            let primary =
                (available_height * DEFAULT_PRIMARY_FRACTION).clamp(0.0, available_height);
            return PaneHeights {
                primary,
                auxiliary: available_height - primary,
            };
        }

        let primary = (available_height * self.primary_fraction)
            .clamp(MIN_PRIMARY_HEIGHT, available_height - MIN_AUXILIARY_HEIGHT);
        PaneHeights {
            primary,
            auxiliary: available_height - primary,
        }
    }

    pub fn resize_primary(&mut self, primary_height: f32, available_height: f32) {
        if available_height <= 0.0 {
            return;
        }
        let clamped = if available_height > MIN_PRIMARY_HEIGHT + MIN_AUXILIARY_HEIGHT {
            primary_height.clamp(MIN_PRIMARY_HEIGHT, available_height - MIN_AUXILIARY_HEIGHT)
        } else {
            primary_height.clamp(0.0, available_height)
        };
        self.primary_fraction = clamped / available_height;
    }

    pub fn reset(&mut self) {
        self.primary_fraction = DEFAULT_PRIMARY_FRACTION;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_split_favors_the_primary_pane() {
        let heights = WorkbenchLayout::default().pane_heights(600.0);
        assert_eq!(
            heights,
            PaneHeights {
                primary: 372.0,
                auxiliary: 228.0
            }
        );
    }

    #[test]
    fn drag_respects_both_minimums() {
        let mut layout = WorkbenchLayout::default();
        layout.resize_primary(590.0, 600.0);
        assert_eq!(layout.pane_heights(600.0).primary, 460.0);
        layout.resize_primary(10.0, 600.0);
        assert_eq!(layout.pane_heights(600.0).primary, 220.0);
    }

    #[test]
    fn reset_restores_the_default_ratio() {
        let mut layout = WorkbenchLayout::default();
        layout.resize_primary(400.0, 600.0);
        layout.reset();
        assert_eq!(layout.pane_heights(600.0).primary, 372.0);
    }
}

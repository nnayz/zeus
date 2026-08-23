//! Pure workbench layout state.
//!
//! GPUI owns pixels and pointer routing; this module owns only the durable
//! split intent. Keeping that seam free of views makes it straightforward to
//! add more pane kinds and a trailing inspector without teaching terminal
//! rendering about workbench policy.

pub const DEFAULT_PRIMARY_FRACTION: f32 = 0.62;
pub const MAX_INSPECTOR_WIDTH: f32 = 720.0;
pub const MIN_TERMINAL_WIDTH: f32 = 320.0;
const MIN_PRIMARY_HEIGHT: f32 = 220.0;
const MIN_AUXILIARY_HEIGHT: f32 = 140.0;

/// The explicit responsive state of the horizontal workbench.
///
/// `Compact` is the short interval where the terminal is held at its useful
/// minimum while the inspector gives up only the space above its own useful
/// minimum. `Narrow` never squeezes the inspector further: it deliberately
/// collapses the inspector and returns that space to the terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HorizontalLayoutState {
    Wide,
    Compact,
    Narrow,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HorizontalLayoutInput {
    pub window_width: f32,
    pub sidebar_visible: bool,
    pub sidebar_width: f32,
    pub inspector_visible: bool,
    pub requested_inspector_width: f32,
    pub inspector_min_width: f32,
    pub terminal_min_width: f32,
    pub mirrored: bool,
}

/// Settled horizontal geometry shared by the flex row and terminal viewport.
///
/// Mirroring affects only `terminal_x`: equivalent standard and mirrored
/// inputs always receive identical widths.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HorizontalLayout {
    state: HorizontalLayoutState,
    pub sidebar_width: f32,
    pub terminal_width: f32,
    pub inspector_width: f32,
    pub terminal_x: f32,
}

impl HorizontalLayout {
    pub const fn inspector_collapsed(self) -> bool {
        matches!(self.state, HorizontalLayoutState::Narrow)
    }

    /// Whether terminal chrome should treat the inspector as present.
    ///
    /// A Narrow auto-collapse keeps the inspector "open" so a reveal button
    /// does not appear for a panel that returns as soon as there is room.
    /// A user-closed inspector is the only case that offers that control.
    pub const fn inspector_chrome_open(self) -> bool {
        self.inspector_width > 0.0 || self.inspector_collapsed()
    }
}

/// Resolves the workbench's horizontal allocation without consulting GPUI.
///
/// The policy is intentionally small and deterministic:
///
/// 1. **Wide:** preserve the requested inspector width; the terminal receives
///    the remainder.
/// 2. **Compact:** preserve both useful minimums. The terminal stays at its
///    minimum while the inspector uses the remainder, never less than its own
///    minimum.
/// 3. **Narrow:** when both minimums cannot fit, collapse the inspector and
///    give the terminal all space after the sidebar.
///
/// A hidden inspector does not participate in the allocation. Sidebar width
/// is kept ahead of terminal/inspector allocation, matching its independently
/// persisted workbench seam.
pub fn solve_horizontal_layout(input: HorizontalLayoutInput) -> HorizontalLayout {
    let window_width = finite_nonnegative(input.window_width);
    let sidebar_width = if input.sidebar_visible {
        finite_nonnegative(input.sidebar_width).min(window_width)
    } else {
        0.0
    };
    let available = (window_width - sidebar_width).max(0.0);
    let terminal_min_width = finite_nonnegative(input.terminal_min_width);

    let (state, inspector_width, terminal_width) = if input.inspector_visible {
        let inspector_min_width = finite_nonnegative(input.inspector_min_width);
        let requested_inspector_width =
            finite_nonnegative(input.requested_inspector_width).max(inspector_min_width);

        if available >= requested_inspector_width + terminal_min_width {
            (
                HorizontalLayoutState::Wide,
                requested_inspector_width,
                available - requested_inspector_width,
            )
        } else if available >= inspector_min_width + terminal_min_width {
            (
                HorizontalLayoutState::Compact,
                available - terminal_min_width,
                terminal_min_width,
            )
        } else {
            (HorizontalLayoutState::Narrow, 0.0, available)
        }
    } else {
        (HorizontalLayoutState::Wide, 0.0, available)
    };

    let terminal_x = if input.mirrored {
        inspector_width
    } else {
        sidebar_width
    };

    HorizontalLayout {
        state,
        sidebar_width,
        terminal_width,
        inspector_width,
        terminal_x,
    }
}

fn finite_nonnegative(value: f32) -> f32 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

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

    const INSPECTOR_MIN: f32 = 374.0;

    fn horizontal_input(window_width: f32) -> HorizontalLayoutInput {
        HorizontalLayoutInput {
            window_width,
            sidebar_visible: true,
            sidebar_width: 220.0,
            inspector_visible: true,
            requested_inspector_width: 400.0,
            inspector_min_width: INSPECTOR_MIN,
            terminal_min_width: MIN_TERMINAL_WIDTH,
            mirrored: false,
        }
    }

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

    #[test]
    fn horizontal_layout_has_explicit_wide_compact_and_narrow_states() {
        let wide = solve_horizontal_layout(horizontal_input(1_400.0));
        assert_eq!(wide.state, HorizontalLayoutState::Wide);
        assert_eq!(wide.sidebar_width, 220.0);
        assert_eq!(wide.inspector_width, 400.0);
        assert_eq!(wide.terminal_width, 780.0);

        let compact = solve_horizontal_layout(horizontal_input(930.0));
        assert_eq!(compact.state, HorizontalLayoutState::Compact);
        assert_eq!(compact.inspector_width, 390.0);
        assert_eq!(compact.terminal_width, MIN_TERMINAL_WIDTH);

        let narrow = solve_horizontal_layout(horizontal_input(900.0));
        assert_eq!(narrow.state, HorizontalLayoutState::Narrow);
        assert!(narrow.inspector_collapsed());
        assert_eq!(narrow.inspector_width, 0.0);
        assert_eq!(narrow.terminal_width, 680.0);
    }

    #[test]
    fn horizontal_layout_matrix_preserves_minimums_or_explicitly_collapses() {
        for window_width in [900.0, 930.0, 1_400.0] {
            for sidebar_visible in [false, true] {
                for inspector_visible in [false, true] {
                    for mirrored in [false, true] {
                        let mut input = horizontal_input(window_width);
                        input.sidebar_visible = sidebar_visible;
                        input.inspector_visible = inspector_visible;
                        input.mirrored = mirrored;
                        let layout = solve_horizontal_layout(input);

                        assert_eq!(
                            layout.sidebar_width + layout.terminal_width + layout.inspector_width,
                            window_width
                        );
                        assert!(
                            layout.inspector_width == 0.0
                                || layout.inspector_width >= INSPECTOR_MIN
                        );
                        assert_eq!(
                            layout.inspector_collapsed(),
                            inspector_visible && layout.inspector_width == 0.0
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn mirrored_and_standard_layouts_have_identical_widths_at_every_breakpoint() {
        for window_width in [913.0, 914.0, 919.0, 920.0, 939.0, 940.0, 941.0, 1_400.0] {
            for sidebar_visible in [false, true] {
                for inspector_visible in [false, true] {
                    let mut standard_input = horizontal_input(window_width);
                    standard_input.sidebar_visible = sidebar_visible;
                    standard_input.inspector_visible = inspector_visible;
                    let standard = solve_horizontal_layout(standard_input);

                    let mirrored = solve_horizontal_layout(HorizontalLayoutInput {
                        mirrored: true,
                        ..standard_input
                    });

                    assert_eq!(standard.state, mirrored.state);
                    assert_eq!(standard.sidebar_width, mirrored.sidebar_width);
                    assert_eq!(standard.terminal_width, mirrored.terminal_width);
                    assert_eq!(standard.inspector_width, mirrored.inspector_width);
                    assert_eq!(standard.terminal_x, standard.sidebar_width);
                    assert_eq!(mirrored.terminal_x, mirrored.inspector_width);
                }
            }
        }
    }

    #[test]
    fn compact_breakpoints_are_inclusive_and_narrow_is_a_clean_collapse() {
        let exact_minimums = solve_horizontal_layout(horizontal_input(914.0));
        assert_eq!(exact_minimums.state, HorizontalLayoutState::Compact);
        assert_eq!(exact_minimums.inspector_width, INSPECTOR_MIN);
        assert_eq!(exact_minimums.terminal_width, MIN_TERMINAL_WIDTH);

        let below_minimums = solve_horizontal_layout(horizontal_input(913.0));
        assert_eq!(below_minimums.state, HorizontalLayoutState::Narrow);
        assert_eq!(below_minimums.inspector_width, 0.0);
        assert_eq!(below_minimums.terminal_width, 693.0);

        let exact_requested = solve_horizontal_layout(horizontal_input(940.0));
        assert_eq!(exact_requested.state, HorizontalLayoutState::Wide);
        assert_eq!(exact_requested.inspector_width, 400.0);
        assert_eq!(exact_requested.terminal_width, MIN_TERMINAL_WIDTH);
    }

    #[test]
    fn hidden_panels_do_not_consume_horizontal_space() {
        let neither = solve_horizontal_layout(HorizontalLayoutInput {
            sidebar_visible: false,
            inspector_visible: false,
            ..horizontal_input(900.0)
        });
        assert_eq!(neither.terminal_width, 900.0);

        let inspector_only = solve_horizontal_layout(HorizontalLayoutInput {
            sidebar_visible: false,
            ..horizontal_input(900.0)
        });
        assert_eq!(inspector_only.inspector_width, 400.0);
        assert_eq!(inspector_only.terminal_width, 500.0);

        let sidebar_only = solve_horizontal_layout(HorizontalLayoutInput {
            inspector_visible: false,
            ..horizontal_input(900.0)
        });
        assert_eq!(sidebar_only.sidebar_width, 220.0);
        assert_eq!(sidebar_only.terminal_width, 680.0);
    }

    #[test]
    fn narrow_auto_collapse_is_distinct_from_a_user_closed_inspector() {
        let auto_hidden = solve_horizontal_layout(horizontal_input(900.0));
        assert!(auto_hidden.inspector_collapsed());
        assert_eq!(auto_hidden.inspector_width, 0.0);
        assert!(auto_hidden.inspector_chrome_open());

        let user_closed = solve_horizontal_layout(HorizontalLayoutInput {
            inspector_visible: false,
            ..horizontal_input(900.0)
        });
        assert!(!user_closed.inspector_collapsed());
        assert_eq!(user_closed.inspector_width, 0.0);
        assert!(!user_closed.inspector_chrome_open());
    }

    struct WorkbenchRowHarness {
        layout: HorizontalLayout,
        mirrored: bool,
    }

    impl gpui::Render for WorkbenchRowHarness {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            use gpui::prelude::*;

            let layout = self.layout;
            let sidebar = (layout.sidebar_width > 0.0).then(|| {
                gpui::div()
                    .debug_selector(|| "workbench-sidebar".into())
                    .flex_none()
                    .w(gpui::px(layout.sidebar_width))
                    .h_full()
                    .bg(gpui::white())
            });
            let terminal = gpui::div()
                .debug_selector(|| "workbench-terminal".into())
                .flex_1()
                .min_w(gpui::px(0.0))
                .h_full()
                .bg(gpui::white());
            let inspector = (layout.inspector_width > 0.0).then(|| {
                gpui::div()
                    .debug_selector(|| "workbench-inspector".into())
                    .flex_none()
                    .w(gpui::px(layout.inspector_width))
                    .h_full()
                    .bg(gpui::white())
            });

            let mut row = gpui::div().flex().size_full();
            if self.mirrored {
                if let Some(inspector) = inspector {
                    row = row.child(inspector);
                }
                row = row.child(terminal);
                if let Some(sidebar) = sidebar {
                    row = row.child(sidebar);
                }
            } else {
                if let Some(sidebar) = sidebar {
                    row = row.child(sidebar);
                }
                row = row.child(terminal);
                if let Some(inspector) = inspector {
                    row = row.child(inspector);
                }
            }
            row
        }
    }

    #[gpui::test]
    fn gpui_row_matches_solver_at_wide_compact_and_narrow_breakpoints(
        cx: &mut gpui::TestAppContext,
    ) {
        let initial = solve_horizontal_layout(horizontal_input(1_400.0));
        let (view, cx) = cx.add_window_view(move |_, _| WorkbenchRowHarness {
            layout: initial,
            mirrored: false,
        });

        for window_width in [900.0, 913.0, 914.0, 930.0, 940.0, 1_400.0] {
            for sidebar_visible in [false, true] {
                for inspector_visible in [false, true] {
                    for mirrored in [false, true] {
                        let mut input = horizontal_input(window_width);
                        input.sidebar_visible = sidebar_visible;
                        input.inspector_visible = inspector_visible;
                        input.mirrored = mirrored;
                        let layout = solve_horizontal_layout(input);
                        view.update(cx, |harness, cx| {
                            harness.layout = layout;
                            harness.mirrored = mirrored;
                            cx.notify();
                        });
                        cx.simulate_resize(gpui::size(gpui::px(window_width), gpui::px(720.0)));

                        let terminal = cx
                            .debug_bounds("workbench-terminal")
                            .expect("terminal column");
                        assert_eq!(terminal.size.width, gpui::px(layout.terminal_width));
                        assert_eq!(terminal.origin.x, gpui::px(layout.terminal_x));

                        match cx.debug_bounds("workbench-inspector") {
                            Some(inspector) => {
                                assert_eq!(inspector.size.width, gpui::px(layout.inspector_width));
                                assert!(inspector.size.width >= gpui::px(INSPECTOR_MIN));
                            }
                            None => assert_eq!(layout.inspector_width, 0.0),
                        }

                        match cx.debug_bounds("workbench-sidebar") {
                            Some(sidebar) => {
                                assert_eq!(sidebar.size.width, gpui::px(layout.sidebar_width))
                            }
                            None => assert_eq!(layout.sidebar_width, 0.0),
                        }
                    }
                }
            }
        }
    }
}

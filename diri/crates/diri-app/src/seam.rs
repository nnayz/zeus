//! Motion for the two panels that flank the workbench.
//!
//! The sidebar and the inspector are mirror images: each is a fixed-width panel
//! whose seam against the terminal card slides open and shut. Both animate the
//! seam width directly rather than transforming the panel, because the card is
//! a flex sibling that has to follow the seam -- so the curve, the duration,
//! and the rule for what a toggle does mid-slide all have to agree between
//! them, and they only agree reliably if they are the same code.

use std::time::{Duration, Instant};

use diri_ui::Motion;

/// How long a panel takes to slide open or shut.
pub const SEAM_SLIDE: Duration = Duration::from_millis(Motion::SEAM_SLIDE_MS);

/// Whether a panel toggle should be honoured, given how long ago the last one
/// was. Toggles arriving mid-slide are dropped rather than queued or reversed:
/// ⌘B and ⌘⇧D autorepeat, and a half-played slide flipping direction under a
/// held key reads as a stutter rather than as a panel. Dropping is also honest
/// -- the user is leaning on the key precisely because the panel has not
/// settled yet, so replaying those presses afterwards would land somewhere they
/// never asked for.
pub fn toggle_has_settled(since_last: Option<Duration>) -> bool {
    since_last.is_none_or(|since| since >= SEAM_SLIDE)
}

/// A panel open or close in flight. It records where the seam left from, in
/// points, and reads its destination back from the settled layout on every
/// frame rather than capturing it -- so a width the user drags out from under a
/// slide still lands where the layout wants it, instead of the slide finishing
/// on a stale target and snapping.
#[derive(Clone, Copy, Debug)]
pub struct SeamSlide {
    from: f32,
    started_at: Instant,
}

impl SeamSlide {
    /// Starts a slide away from `from`, unless there is nowhere to travel.
    pub fn begin(from: f32, to: f32) -> Option<Self> {
        (from != to).then(|| Self {
            from,
            started_at: Instant::now(),
        })
    }

    pub fn progress(&self, now: Instant) -> f32 {
        (now.duration_since(self.started_at).as_secs_f32() / SEAM_SLIDE.as_secs_f32())
            .clamp(0.0, 1.0)
    }

    pub fn is_done(&self, now: Instant) -> bool {
        self.progress(now) >= 1.0
    }

    pub fn seam_at(&self, to: f32, now: Instant) -> f32 {
        self.from + (to - self.from) * spring_settle(self.progress(now))
    }
}

/// The step response of a critically damped spring, normalised to span exactly
/// 0..1. It leaves fast and settles soft, which is the "spring" part -- but it
/// never crosses its target, unlike the overshooting curves that usually carry
/// that name. Overshoot is wrong here specifically: the seam pushes the
/// terminal card, so a bounce past the target would drag the whole workbench
/// back and forth rather than reading as elasticity on one small control.
fn spring_settle(delta: f32) -> f32 {
    const STIFFNESS: f32 = 7.0;
    let remaining = |t: f32| (1.0 + STIFFNESS * t) * (-STIFFNESS * t).exp();
    // The raw response is still ~0.7% short at t = 1. Divide that out, or the
    // slide ends on a visible one-frame snap onto the settled width.
    (1.0 - remaining(delta)) / (1.0 - remaining(1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_slide_spans_its_whole_range() {
        assert_eq!(spring_settle(0.0), 0.0);
        assert_eq!(spring_settle(1.0), 1.0);
    }

    #[test]
    fn the_slide_never_overshoots_or_backs_up() {
        // Overshoot here would shove the terminal card past its resting edge
        // and drag it back; a non-monotonic curve would read as a stutter.
        let mut previous = 0.0;
        for step in 0..=100 {
            let value = spring_settle(step as f32 / 100.0);
            assert!(
                (0.0..=1.0).contains(&value),
                "spring_settle({step}/100) left 0..=1 at {value}"
            );
            assert!(value >= previous, "spring_settle went backwards at {step}");
            previous = value;
        }
    }

    #[test]
    fn the_slide_front_loads_its_travel() {
        // The "spring" read comes from covering most of the distance early and
        // easing into the target, rather than moving linearly.
        assert!(spring_settle(0.5) > 0.8, "{}", spring_settle(0.5));
    }

    #[test]
    fn a_seam_with_nowhere_to_go_never_starts_a_slide() {
        assert!(SeamSlide::begin(248.0, 248.0).is_none());
        assert!(SeamSlide::begin(0.0, 248.0).is_some());
    }

    #[test]
    fn a_closing_slide_lands_on_a_shut_seam() {
        let slide = SeamSlide {
            from: 248.0,
            started_at: Instant::now() - SEAM_SLIDE,
        };
        let now = Instant::now();
        assert!(slide.is_done(now));
        assert_eq!(slide.seam_at(0.0, now), 0.0);
    }

    #[test]
    fn a_slide_that_has_not_started_paints_its_origin() {
        let started_at = Instant::now();
        let slide = SeamSlide {
            from: 0.0,
            started_at,
        };
        assert_eq!(slide.seam_at(248.0, started_at), 0.0);
    }

    #[test]
    fn a_slide_retargets_when_the_seam_is_dragged_out_from_under_it() {
        // Half a slide into opening toward 248, the user drags the seam to 300.
        let slide = SeamSlide {
            from: 0.0,
            started_at: Instant::now() - SEAM_SLIDE / 2,
        };
        let settled = Instant::now() + SEAM_SLIDE;
        assert_eq!(slide.seam_at(300.0, settled), 300.0);
    }

    #[test]
    fn the_first_toggle_is_always_honoured() {
        assert!(toggle_has_settled(None));
    }

    #[test]
    fn a_repeating_shortcut_cannot_outrun_the_slide() {
        // ⌘B autorepeats around every 30ms once held.
        assert!(!toggle_has_settled(Some(Duration::from_millis(30))));
        assert!(!toggle_has_settled(Some(
            SEAM_SLIDE - Duration::from_millis(1)
        )));
    }

    #[test]
    fn a_toggle_lands_again_once_the_slide_finishes() {
        assert!(toggle_has_settled(Some(SEAM_SLIDE)));
        assert!(toggle_has_settled(Some(Duration::from_secs(1))));
    }
}

//! Scene-level backdrop blur for floating glass surfaces.
//!
//! The child subtree is painted in one layer so the blur always lands before
//! its tint, border, controls, and text. This ordering is essential: painting
//! the blur as an unrelated primitive can cause later hover damage to blur the
//! card's own content. Adapted from Zeron's MIT-licensed frost element.

use gpui::{
    AnyElement, App, Bounds, Corners, Element, GlobalElementId, InspectorElementId, IntoElement,
    LayoutId, Pixels, Window, px,
};

pub const MENU_BLUR: f32 = 8.0;

pub struct Frosted {
    corner_radius: f32,
    blur_radius: f32,
    child: AnyElement,
}

impl Frosted {
    pub fn new(corner_radius: f32, blur_radius: f32, child: impl IntoElement) -> Self {
        Self {
            corner_radius,
            blur_radius,
            child: child.into_any_element(),
        }
    }
}

impl Element for Frosted {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<gpui::ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        window.paint_layer(bounds, |window| {
            window.paint_backdrop_blur(
                bounds,
                Corners::all(px(self.corner_radius)),
                px(self.blur_radius),
            );
            self.child.paint(window, cx);
        });
    }
}

impl IntoElement for Frosted {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

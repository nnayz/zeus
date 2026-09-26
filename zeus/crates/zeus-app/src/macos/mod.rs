pub mod brand_raster;
pub mod menu_bar;
pub mod notifier;
pub mod sf_symbols;
mod window_blur;

use objc2_foundation::NSBundle;

pub(crate) fn install_softer_window_blur() {
    window_blur::install();
}

pub(crate) fn bundle_identifier() -> Option<String> {
    NSBundle::mainBundle()
        .bundleIdentifier()
        .map(|identifier| identifier.to_string())
}

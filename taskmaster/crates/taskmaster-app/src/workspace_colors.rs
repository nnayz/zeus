//! Neutral workspace surfaces from DESIGN.md.

use gpui::{Rgba, rgb};
use taskmaster_ui::Appearance;

/// The neutral desktop navigation palette specified in DESIGN.md. Deliberately
/// scoped to workspace navigation so terminal theme colors retain their meaning.
#[derive(Clone, Copy)]
pub(crate) struct WorkspaceColors {
    pub(crate) foundation: Rgba,
    pub(crate) surface: Rgba,
    pub(crate) raised: Rgba,
    pub(crate) hover: Rgba,
    pub(crate) outline: Rgba,
    pub(crate) primary: Rgba,
    pub(crate) secondary: Rgba,
    pub(crate) focus: Rgba,
}

impl WorkspaceColors {
    pub(crate) fn new(appearance: Appearance) -> Self {
        match appearance {
            Appearance::Light => Self {
                foundation: rgb(0xfafafa),
                surface: rgb(0xfcfcfc),
                raised: rgb(0xffffff),
                hover: rgb(0xe8e8e8),
                outline: rgb(0xededed),
                primary: rgb(0x191919),
                secondary: rgb(0x6e6e6e),
                focus: rgb(0x006bd6),
            },
            Appearance::Dark => Self {
                foundation: rgb(0x080808),
                surface: rgb(0x0d0d0d),
                raised: rgb(0x161616),
                hover: rgb(0x1a1a1a),
                outline: rgb(0x272725),
                primary: rgb(0xe6e6e6),
                secondary: rgb(0xa0a0a0),
                focus: rgb(0x2997ff),
            },
        }
    }
}

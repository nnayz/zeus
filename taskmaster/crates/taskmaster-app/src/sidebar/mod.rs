//! Window-sidebar state, deterministic workbench preview data, and GPUI rendering.

mod fixture;
mod state;
mod view;

pub use fixture::{PreviewScenario, SidebarPreviewFixture};
pub use state::{DragItem, Popover, SidebarUiState, move_before, move_to_end};
pub(crate) use view::validate_workspace_root;
pub use view::{Sidebar, SidebarEvent};

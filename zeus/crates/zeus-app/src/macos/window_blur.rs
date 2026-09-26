//! Pull Zeron's window blur back down.
//!
//! The pinned renderer forces a 60px gaussian on the behind-window effect
//! every time the blurred view updates its layer. That radius is private to
//! the renderer, so the replacement runs after the original update and writes
//! a shorter radius back onto the same filters.

use std::mem;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};

use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
use objc2::{msg_send, sel};
use objc2_foundation::NSString;

/// Desktop gaussian under the window frost. The renderer writes 60.
const WINDOW_BLUR_RADIUS: f64 = 32.0;

static ORIGINAL_UPDATE_LAYER: AtomicPtr<()> = AtomicPtr::new(std::ptr::null_mut());
static INSTALLED: AtomicBool = AtomicBool::new(false);

pub(crate) fn install() {
    if INSTALLED.swap(true, Ordering::SeqCst) {
        return;
    }
    let Some(class) = AnyClass::get(c"BlurredView") else {
        INSTALLED.store(false, Ordering::SeqCst);
        return;
    };
    let Some(method) = class.instance_method(sel!(updateLayer)) else {
        INSTALLED.store(false, Ordering::SeqCst);
        return;
    };
    let replacement: Imp = unsafe {
        mem::transmute::<unsafe extern "C-unwind" fn(&AnyObject, Sel), Imp>(softer_update_layer)
    };
    let previous = unsafe { method.set_implementation(replacement) };
    ORIGINAL_UPDATE_LAYER.store(previous as *mut (), Ordering::SeqCst);
}

unsafe extern "C-unwind" fn softer_update_layer(this: &AnyObject, cmd: Sel) {
    let original = ORIGINAL_UPDATE_LAYER.load(Ordering::SeqCst);
    if !original.is_null() {
        let original: unsafe extern "C-unwind" fn(&AnyObject, Sel) =
            unsafe { mem::transmute(original) };
        unsafe { original(this, cmd) };
    }
    unsafe { retune_view(this) };
}

unsafe fn retune_view(view: &AnyObject) {
    let layer: *mut AnyObject = unsafe { msg_send![view, layer] };
    unsafe { retune_layer(layer) };
}

unsafe fn retune_layer(layer: *mut AnyObject) {
    if layer.is_null() {
        return;
    }
    unsafe {
        let filters: *mut AnyObject = msg_send![layer, filters];
        if !filters.is_null() {
            let count: usize = msg_send![filters, count];
            let mut changed = false;
            for index in 0..count {
                let filter: *mut AnyObject = msg_send![filters, objectAtIndex: index];
                if filter.is_null() || !filter_is_blur(filter) {
                    continue;
                }
                let Some(nsnumber) = AnyClass::get(c"NSNumber") else {
                    return;
                };
                let radius: *mut AnyObject =
                    msg_send![nsnumber, numberWithDouble: WINDOW_BLUR_RADIUS];
                let key = NSString::from_str("inputRadius");
                let _: () = msg_send![filter, setValue: radius, forKey: &*key];
                changed = true;
            }
            if changed {
                // The layer copies its filter list into the render tree.
                let _: () = msg_send![layer, setFilters: filters];
            }
        }

        let sublayers: *mut AnyObject = msg_send![layer, sublayers];
        if sublayers.is_null() {
            return;
        }
        let count: usize = msg_send![sublayers, count];
        for index in 0..count {
            let sublayer: *mut AnyObject = msg_send![sublayers, objectAtIndex: index];
            retune_layer(sublayer);
        }
    }
}

unsafe fn filter_is_blur(filter: *mut AnyObject) -> bool {
    let description: *const NSString = unsafe { msg_send![filter, description] };
    if description.is_null() {
        return false;
    }
    unsafe { &*description }.to_string().contains("Blur")
}

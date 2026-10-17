//! A main-thread WebKit surface mounted in a GPUI panel's page bounds.
//!
//! WebKit is an OS framework; dynamic selectors keep this bridge within the
//! Objective-C bindings already shipped by Taskmaster. The NSView belongs to one
//! Browser tab and is removed from the window whenever that tab is hidden.

use block2::RcBlock;
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::NSView;
use objc2_foundation::{
    MainThreadMarker, NSError, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use tokio::sync::mpsc;

use crate::browser_panel::BrowserState;
use gpui::{Bounds, Pixels, Window};
use taskmaster_ui::Appearance;

#[link(name = "WebKit", kind = "framework")]
unsafe extern "C" {
    // A real class reference keeps WebKit linked when release builds strip
    // unused dylibs; dynamic selector names alone do not create such a use.
    #[link_name = "OBJC_CLASS_$_WKWebView"]
    static WEB_VIEW_CLASS: AnyClass;
}

#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {
    #[link_name = "NSAppearanceNameAqua"]
    static AQUA_APPEARANCE: &'static NSString;
    #[link_name = "NSAppearanceNameDarkAqua"]
    static DARK_AQUA_APPEARANCE: &'static NSString;
}

pub(crate) struct NativeBrowser {
    view: Retained<NSView>,
    // WKWebView's navigationDelegate property is weak.
    _delegate: Retained<BrowserDelegate>,
    mounted_parent: Option<Retained<NSView>>,
}

impl NativeBrowser {
    pub(crate) fn new(sender: mpsc::UnboundedSender<BrowserState>) -> Result<Self, String> {
        let mtm = MainThreadMarker::new().ok_or("Browser must run on the main thread.")?;
        // SAFETY: WKWebView is a public WebKit class present on every macOS
        // version Taskmaster supports. Objective-C class objects are immutable.
        let web_class = unsafe { &WEB_VIEW_CLASS };
        let config_class =
            AnyClass::get(c"WKWebViewConfiguration").ok_or("WebKit is unavailable.")?;
        let data_class = AnyClass::get(c"WKWebsiteDataStore").ok_or("WebKit is unavailable.")?;
        // SAFETY: The class selectors and argument types are the documented
        // WebKit API. Every call is on the AppKit main thread. A fresh private
        // data store ensures this panel never persists cookies or page cache.
        let view = unsafe {
            let configuration: Retained<AnyObject> = msg_send![config_class, new];
            let data_store: Retained<AnyObject> = msg_send![data_class, nonPersistentDataStore];
            let _: () = msg_send![&*configuration, setWebsiteDataStore: &*data_store];
            let allocated: Allocated<NSView> = msg_send![web_class, alloc];
            let view: Retained<NSView> = msg_send![allocated,
                initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1.0, 1.0)),
                configuration: &*configuration
            ];
            let _: () = msg_send![&*view, setAllowsBackForwardNavigationGestures: true];
            view
        };
        let delegate = BrowserDelegate::new(mtm, sender);
        // SAFETY: BrowserDelegate implements WebKit's documented delegate
        // selectors and stays retained until after the view is disconnected.
        unsafe {
            let _: () = msg_send![&*view, setNavigationDelegate: &*delegate];
        }
        Ok(Self {
            view,
            _delegate: delegate,
            mounted_parent: None,
        })
    }

    pub(crate) fn mount(&mut self, window: &Window, bounds: Bounds<Pixels>) -> Result<(), String> {
        let handle = HasWindowHandle::window_handle(window)
            .map_err(|_| "Browser cannot attach to this window.")?;
        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
            return Err("Browser requires an AppKit window.".into());
        };
        // SAFETY: GPUI's raw-window-handle contract exposes its live native
        // NSView. Mount is called only while that Window is being painted on
        // the main thread; retaining the parent protects detach during drop.
        let parent = unsafe { Retained::retain(handle.ns_view.as_ptr().cast::<NSView>()) }
            .ok_or("Browser cannot attach to this window.")?;
        let y = if parent.isFlipped() {
            f64::from(bounds.top())
        } else {
            parent.bounds().size.height - f64::from(bounds.bottom())
        };
        let frame = NSRect::new(
            NSPoint::new(f64::from(bounds.left()), y),
            NSSize::new(f64::from(bounds.size.width), f64::from(bounds.size.height)),
        );
        self.view.setFrame(frame);
        if self
            .mounted_parent
            .as_ref()
            .is_none_or(|mounted| !std::ptr::eq(&**mounted, &*parent))
        {
            self.hide();
            parent.addSubview(&self.view);
            self.mounted_parent = Some(parent);
        }
        self.view.setHidden(false);
        Ok(())
    }

    pub(crate) fn set_appearance(&self, appearance: Appearance) {
        let Some(appearance_class) = AnyClass::get(c"NSAppearance") else {
            return;
        };
        // SAFETY: These are AppKit's exported NSString constants and the
        // documented NSAppearance factory/property on main-thread NSViews.
        unsafe {
            let name = match appearance {
                Appearance::Light => AQUA_APPEARANCE,
                Appearance::Dark => DARK_AQUA_APPEARANCE,
            };
            let native: Option<Retained<AnyObject>> =
                msg_send![appearance_class, appearanceNamed: name];
            let _: () = msg_send![&*self.view, setAppearance: native.as_deref()];
        }
    }

    pub(crate) fn hide(&mut self) {
        self.return_focus();
        self.view.setHidden(true);
        self.view.removeFromSuperview();
        self.mounted_parent = None;
    }

    pub(crate) fn return_focus(&self) {
        let Some(parent) = &self.mounted_parent else {
            return;
        };
        let Some(window) = self.view.window() else {
            return;
        };
        if self.is_focused() {
            window.makeFirstResponder(Some(parent));
        }
    }

    pub(crate) fn is_focused(&self) -> bool {
        self.view
            .window()
            .and_then(|window| window.firstResponder())
            .is_some_and(|responder| {
                responder
                    .downcast_ref::<NSView>()
                    .is_some_and(|view| view.isDescendantOf(&self.view))
            })
    }

    pub(crate) fn check_preview_visibility(&self, visible: bool) -> Result<(), String> {
        let valid = if visible {
            self.mounted_parent.is_some() && self.view.window().is_some() && !self.view.isHidden()
        } else {
            // SAFETY: The retained view and its parent are inspected only on
            // AppKit's main thread, without mutating the view hierarchy.
            self.mounted_parent.is_none()
                && unsafe { self.view.superview().is_none() }
                && self.view.isHidden()
        };
        if valid {
            Ok(())
        } else {
            Err(format!(
                "Browser native visibility did not become {visible}."
            ))
        }
    }

    pub(crate) fn navigate(&self, url: &str) -> Result<(), String> {
        let url_class = AnyClass::get(c"NSURL").ok_or("URL services are unavailable.")?;
        let request_class =
            AnyClass::get(c"NSURLRequest").ok_or("URL services are unavailable.")?;
        // SAFETY: URL input was normalized to http/https before this method.
        // NSURL and NSURLRequest use documented Foundation selectors.
        unsafe {
            let ns_url: Option<Retained<AnyObject>> =
                msg_send![url_class, URLWithString: &*NSString::from_str(url)];
            let ns_url = ns_url.ok_or("Enter a valid website address.")?;
            let request: Retained<AnyObject> = msg_send![request_class, requestWithURL: &*ns_url];
            let _: Option<Retained<AnyObject>> = msg_send![&*self.view, loadRequest: &*request];
        }
        Ok(())
    }

    pub(crate) fn back(&self) {
        // SAFETY: WKWebView's documented selector; calls remain main-thread.
        unsafe {
            let _: Option<Retained<AnyObject>> = msg_send![&*self.view, goBack];
        }
    }

    pub(crate) fn load_preview_html(&self) {
        // This document is completely local: no scripts, fonts, images, or
        // other network resources. It exercises WebKit rendering/delegates.
        const HTML: &str = r#"<!doctype html><html><head><meta name="viewport" content="width=device-width,initial-scale=1"><title>Taskmaster browser preview</title><style>
            :root { color-scheme: light dark; font: 14px -apple-system, BlinkMacSystemFont, sans-serif; background: #fcfcfc; color: #191919; }
            body { margin: 0; min-height: 100vh; display: grid; place-items: center; }
            main { max-width: 320px; padding: 24px; } h1 { font-size: 18px; font-weight: 500; margin: 0 0 12px; }
            p { color: #6e6e6e; line-height: 1.5; margin: 8px 0; } .status { margin-top: 20px; font-size: 12px; }
            @media (prefers-color-scheme: dark) { :root { background: #0d0d0d; color: #e6e6e6; } p { color: #a0a0a0; } }
            </style></head><body><main><h1>Browser</h1><p>Web pages appear inside this panel.</p><p>Use the address bar for a website or a local development server.</p><p class="status">Local preview · no network requests</p></main></body></html>"#;
        // SAFETY: WKWebView's documented loadHTMLString:baseURL: method takes
        // NSString plus an optional NSURL. A nil base cannot resolve resources.
        unsafe {
            let _: Option<Retained<AnyObject>> = msg_send![&*self.view,
                loadHTMLString: &*NSString::from_str(HTML),
                baseURL: None::<&AnyObject>
            ];
        }
    }

    /// Exercise the actual AppKit command path in the explicitly opted-in
    /// offline preview. Queue the event after GPUI releases its App borrow.
    pub(crate) fn queue_preview_command_o(&self) -> Result<(), String> {
        let state = state_for_view(&self.view, None);
        if state.title.as_deref() != Some("Taskmaster browser preview") || state.loading {
            return Err("The offline browser preview is still loading.".into());
        }
        let window = self
            .view
            .window()
            .ok_or("The browser preview is not mounted.")?;
        if self.view.isHidden()
            || self.view.bounds().size.width < 2.0
            || self.view.bounds().size.height < 2.0
        {
            return Err("The browser preview has no visible page bounds.".into());
        }
        window.makeFirstResponder(Some(&self.view));
        let event_class = AnyClass::get(c"NSEvent").ok_or("AppKit events are unavailable.")?;
        let queue_class =
            AnyClass::get(c"NSOperationQueue").ok_or("AppKit queue is unavailable.")?;
        let application_class = AnyClass::get(c"NSApplication").ok_or("AppKit is unavailable.")?;
        // SAFETY: This constructs one documented NSEvent keyDown for Command-O
        // scoped to the preview's NSWindow number. The main operation queue
        // dispatches after the current GPUI callback ends, avoiding reentry
        // into a borrowed GPUI App while AppKit routes the command.
        unsafe {
            let event: Option<Retained<AnyObject>> = msg_send![event_class,
                keyEventWithType: 10_usize,
                location: NSPoint::new(0.0, 0.0),
                modifierFlags: 1_usize << 20,
                timestamp: 0.0_f64,
                windowNumber: window.windowNumber(),
                context: None::<&AnyObject>,
                characters: &*NSString::from_str("o"),
                charactersIgnoringModifiers: &*NSString::from_str("o"),
                isARepeat: false,
                keyCode: 31_u16
            ];
            let event = event.ok_or("Could not construct the preview command.")?;
            let application: Retained<AnyObject> = msg_send![application_class, sharedApplication];
            let queue: Retained<AnyObject> = msg_send![queue_class, mainQueue];
            let view = self.view.clone();
            let operation = RcBlock::new(move || {
                window.makeFirstResponder(Some(&view));
                let _: () = msg_send![&*application, sendEvent: &*event];
            });
            let _: () = msg_send![&*queue, addOperationWithBlock: &*operation];
        }
        Ok(())
    }

    pub(crate) fn forward(&self) {
        // SAFETY: WKWebView's documented selector; calls remain main-thread.
        unsafe {
            let _: Option<Retained<AnyObject>> = msg_send![&*self.view, goForward];
        }
    }

    pub(crate) fn reload(&self) {
        // SAFETY: WKWebView's documented selector; calls remain main-thread.
        unsafe {
            let _: Option<Retained<AnyObject>> = msg_send![&*self.view, reload];
        }
    }

    pub(crate) fn stop(&self) {
        // SAFETY: WKWebView's documented selector; calls remain main-thread.
        unsafe {
            let _: () = msg_send![&*self.view, stopLoading];
        }
    }
}

impl Drop for NativeBrowser {
    fn drop(&mut self) {
        self.hide();
        self.stop();
        // SAFETY: Detach the weak delegate before its retained owner drops.
        unsafe {
            let _: () = msg_send![&*self.view, setNavigationDelegate: None::<&AnyObject>];
        }
    }
}

struct DelegateIvars {
    sender: mpsc::UnboundedSender<BrowserState>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements. WebKit dispatches
    // these navigation callbacks on the main thread; there is no custom Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = DelegateIvars]
    struct BrowserDelegate;

    unsafe impl NSObjectProtocol for BrowserDelegate {}

    impl BrowserDelegate {
        #[unsafe(method(webView:didStartProvisionalNavigation:))]
        fn started(&self, view: &AnyObject, _navigation: Option<&AnyObject>) {
            let mut state = state_for_view(view, None);
            state.loading = true;
            let _ = self.ivars().sender.send(state);
        }

        #[unsafe(method(webView:didFinishNavigation:))]
        fn finished(&self, view: &AnyObject, _navigation: Option<&AnyObject>) {
            let mut state = state_for_view(view, None);
            state.loading = false;
            let _ = self.ivars().sender.send(state);
        }

        #[unsafe(method(webView:didFailProvisionalNavigation:withError:))]
        fn failed_provisional(&self, view: &AnyObject, _navigation: Option<&AnyObject>, error: &NSError) {
            send_failure(&self.ivars().sender, view, error);
        }

        #[unsafe(method(webView:didFailNavigation:withError:))]
        fn failed(&self, view: &AnyObject, _navigation: Option<&AnyObject>, error: &NSError) {
            send_failure(&self.ivars().sender, view, error);
        }

        #[unsafe(method(webViewWebContentProcessDidTerminate:))]
        fn terminated(&self, view: &AnyObject) {
            let _ = self.ivars().sender.send(state_for_view(view, Some("The webpage stopped responding. Reload to try again.".into())));
        }
    }
);

impl BrowserDelegate {
    fn new(mtm: MainThreadMarker, sender: mpsc::UnboundedSender<BrowserState>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(DelegateIvars { sender });
        // SAFETY: Initialize the allocated NSObject superclass exactly once.
        unsafe { msg_send![super(this), init] }
    }
}

fn send_failure(sender: &mpsc::UnboundedSender<BrowserState>, view: &AnyObject, error: &NSError) {
    // -999 is ordinary cancellation when a new navigation replaces an older
    // one, or the user presses Stop. Do not replace the existing page.
    if error.code() == -999 {
        return;
    }
    let mut state = state_for_view(view, Some(error.localizedDescription().to_string()));
    state.loading = false;
    let _ = sender.send(state);
}

fn state_for_view(view: &AnyObject, error: Option<String>) -> BrowserState {
    // SAFETY: All callers receive a WKWebView on the main thread. These are
    // its documented read-only properties. No pointers escape this function.
    unsafe {
        let ns_url: Option<Retained<AnyObject>> = msg_send![view, URL];
        let url = ns_url.and_then(|url| {
            let text: Option<Retained<NSString>> = msg_send![&*url, absoluteString];
            text.map(|text| text.to_string())
        });
        let title: Option<Retained<NSString>> = msg_send![view, title];
        BrowserState {
            url,
            title: title.map(|text| text.to_string()),
            loading: msg_send![view, isLoading],
            can_go_back: msg_send![view, canGoBack],
            can_go_forward: msg_send![view, canGoForward],
            error,
        }
    }
}

#[cfg(target_os = "macos")]
pub fn configure_window(window: &winit::window::Window) {
    use objc2::msg_send;
    use objc2::runtime::{AnyClass, AnyObject};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    // NSWindowStyleMask flags.
    const TITLED: usize = 1 << 0;
    const CLOSABLE: usize = 1 << 1;
    const MINIATURIZABLE: usize = 1 << 2;
    const RESIZABLE: usize = 1 << 3;
    const FULLSIZE_CONTENT_VIEW: usize = 1 << 15;

    // NSWindowTitleVisibility::Hidden.
    const TITLE_HIDDEN: isize = 1;

    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::AppKit(h) = handle.as_raw() else {
        return;
    };

    unsafe {
        let view: *mut AnyObject = h.ns_view.as_ptr().cast();
        let nswindow: *mut AnyObject = msg_send![view, window];
        if nswindow.is_null() {
            return;
        }
        let style_mask = TITLED | CLOSABLE | MINIATURIZABLE | RESIZABLE | FULLSIZE_CONTENT_VIEW;
        let _: () = msg_send![nswindow, setStyleMask: style_mask];
        let _: () = msg_send![nswindow, setTitlebarAppearsTransparent: true];
        let _: () = msg_send![nswindow, setTitleVisibility: TITLE_HIDDEN];
        let _: () = msg_send![nswindow, setMovableByWindowBackground: true];

        // Hide close, minimize, zoom buttons.
        for i in 0_isize..3 {
            let button: *mut AnyObject = msg_send![nswindow, standardWindowButton: i];
            if !button.is_null() {
                let _: () = msg_send![button, setHidden: true];
            }
        }

        if let Some(app_class) = AnyClass::get(c"NSApplication") {
            let app: *mut AnyObject = msg_send![app_class, sharedApplication];
            if !app.is_null() {
                let _: () = msg_send![app, activateIgnoringOtherApps: true];
                let _: () = msg_send![nswindow, makeKeyAndOrderFront: app];
            }
        }
    }
}

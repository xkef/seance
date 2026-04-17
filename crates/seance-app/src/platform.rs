#[cfg(target_os = "macos")]
pub fn configure_window(window: &winit::window::Window) {
    use objc2::msg_send;
    use objc2::runtime::{AnyClass, AnyObject};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let handle = window.window_handle().expect("no window handle");
    let nsview = match handle.as_raw() {
        RawWindowHandle::AppKit(h) => h.ns_view.as_ptr(),
        _ => return,
    };
    unsafe {
        let view: *mut AnyObject = nsview.cast();
        let nswindow: *mut AnyObject = msg_send![view, window];
        if nswindow.is_null() {
            return;
        }
        let mask: usize = 1 | 2 | 4 | 8 | (1 << 15);
        let _: () = msg_send![nswindow, setStyleMask: mask];
        let _: () = msg_send![nswindow, setTitlebarAppearsTransparent: true];
        let _: () = msg_send![nswindow, setTitleVisibility: 1_isize];
        let _: () = msg_send![nswindow, setMovableByWindowBackground: true];

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

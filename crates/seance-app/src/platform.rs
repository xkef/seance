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

    // NSTitlebarSeparatorStyle::None — suppresses the 1 px hairline AppKit
    // otherwise draws at the titlebar/content boundary when the content view
    // uses FULLSIZE_CONTENT_VIEW.
    const TITLEBAR_SEPARATOR_NONE: isize = 1;

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
        let _: () = msg_send![
            nswindow,
            setTitlebarSeparatorStyle: TITLEBAR_SEPARATOR_NONE
        ];

        // Hide close, minimize, zoom buttons.
        for i in 0_isize..3 {
            let button: *mut AnyObject = msg_send![nswindow, standardWindowButton: i];
            if !button.is_null() {
                let _: () = msg_send![button, setHidden: true];
            }
        }

        // Hide the `NSTitlebarContainerView` inside the window's theme frame.
        // `titlebarAppearsTransparent` + `fullSizeContentView` leave this view
        // in the hierarchy, and it paints a 1 px highlight at the top of the
        // window regardless of `titlebarSeparatorStyle`. Mirrors Ghostty's
        // `HiddenTitlebarTerminalWindow.reapplyHiddenStyle`.
        if let Some(titlebar_cls) = AnyClass::get(c"NSTitlebarContainerView") {
            let content_view: *mut AnyObject = msg_send![nswindow, contentView];
            if !content_view.is_null() {
                let theme_frame: *mut AnyObject = msg_send![content_view, superview];
                if !theme_frame.is_null() {
                    let subviews: *mut AnyObject = msg_send![theme_frame, subviews];
                    if !subviews.is_null() {
                        let count: usize = msg_send![subviews, count];
                        for i in 0..count {
                            let sub: *mut AnyObject = msg_send![subviews, objectAtIndex: i];
                            let is_titlebar: bool = msg_send![sub, isKindOfClass: titlebar_cls];
                            if is_titlebar {
                                let _: () = msg_send![sub, setHidden: true];
                                break;
                            }
                        }
                    }
                }
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

/// Push the macOS "option-as-alt" policy down into winit's NSView, which
/// drives whether `event.text` contains the Option-composed glyph
/// (e.g. CH `Opt+n` → `~`) or the raw un-composed key (e.g. `n` with ALT
/// modifier set). Without this call, winit routes Option through
/// NSTextInputClient's dead-key processing and `event.text` is empty for
/// dead keys.
#[cfg(target_os = "macos")]
pub fn set_option_as_alt(window: &winit::window::Window, mode: seance_input::OptionAsAlt) {
    use seance_input::OptionAsAlt;
    use winit::platform::macos::{OptionAsAlt as WinitOptionAsAlt, WindowExtMacOS};

    let winit_mode = match mode {
        OptionAsAlt::None => WinitOptionAsAlt::None,
        OptionAsAlt::Left => WinitOptionAsAlt::OnlyLeft,
        OptionAsAlt::Right => WinitOptionAsAlt::OnlyRight,
        OptionAsAlt::Both => WinitOptionAsAlt::Both,
    };
    window.set_option_as_alt(winit_mode);
}

#[cfg(not(target_os = "macos"))]
pub fn set_option_as_alt(_window: &winit::window::Window, _mode: seance_input::OptionAsAlt) {}

/// Apply macOS-only event-loop settings before `build()` (activation policy,
/// menubar role). No-op on other platforms.
#[cfg(target_os = "macos")]
pub fn configure_event_loop<T: 'static>(builder: &mut winit::event_loop::EventLoopBuilder<T>) {
    use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
    builder.with_activation_policy(ActivationPolicy::Regular);
}

#[cfg(not(target_os = "macos"))]
pub fn configure_event_loop<T: 'static>(_builder: &mut winit::event_loop::EventLoopBuilder<T>) {}

/// Apply the window-chrome preset to the native NSWindow. `System` restores
/// the opaque native titlebar and standard buttons; `Hidden`/`ButtonsOnly`
/// give a full-size-content window whose titlebar treatment follows
/// `titlebar_style`, differing only in whether the traffic-light buttons
/// stay visible.
#[cfg(target_os = "macos")]
pub fn configure_window(
    window: &winit::window::Window,
    decoration: seance_config::WindowDecoration,
    titlebar_style: seance_config::TitlebarStyle,
) {
    use objc2::MainThreadMarker;
    use objc2::runtime::{AnyClass, NSObjectProtocol};
    use objc2_app_kit::{
        NSApplication, NSTitlebarSeparatorStyle, NSView, NSWindowStyleMask, NSWindowTitleVisibility,
    };
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use seance_config::{TitlebarStyle, WindowDecoration};

    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::AppKit(h) = handle.as_raw() else {
        return;
    };
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };

    // SAFETY: winit guarantees h.ns_view points to a live NSView for the
    // duration of this call. We're on the main thread (mtm above), so no
    // concurrent mutation of the view hierarchy is possible.
    let view: &NSView = unsafe { h.ns_view.cast::<NSView>().as_ref() };
    let Some(nswindow) = view.window() else {
        return;
    };

    let base_mask = NSWindowStyleMask::Titled
        | NSWindowStyleMask::Closable
        | NSWindowStyleMask::Miniaturizable
        | NSWindowStyleMask::Resizable;

    match decoration {
        WindowDecoration::System => {
            // Native chrome. Reverse any hidden-titlebar styling so a config
            // edit toward `system` isn't left half-applied before a restart.
            nswindow.setStyleMask(base_mask);
            nswindow.setTitlebarAppearsTransparent(false);
            nswindow.setTitleVisibility(NSWindowTitleVisibility::Visible);
            nswindow.setTitlebarSeparatorStyle(NSTitlebarSeparatorStyle::Automatic);
            set_standard_buttons_hidden(&nswindow, false);
        }
        WindowDecoration::Hidden | WindowDecoration::ButtonsOnly => {
            // `Native` titlebar keeps an opaque strip; the other styles draw
            // transparent and extend the content view under the titlebar.
            let native_titlebar = matches!(titlebar_style, TitlebarStyle::Native);
            let mask = if native_titlebar {
                base_mask
            } else {
                base_mask | NSWindowStyleMask::FullSizeContentView
            };
            nswindow.setStyleMask(mask);
            nswindow.setTitlebarAppearsTransparent(!native_titlebar);
            nswindow.setTitleVisibility(if native_titlebar {
                NSWindowTitleVisibility::Visible
            } else {
                NSWindowTitleVisibility::Hidden
            });
            nswindow.setTitlebarSeparatorStyle(if native_titlebar {
                NSTitlebarSeparatorStyle::Automatic
            } else {
                NSTitlebarSeparatorStyle::None
            });

            let hide_buttons = matches!(decoration, WindowDecoration::Hidden);
            set_standard_buttons_hidden(&nswindow, hide_buttons);

            // The private `NSTitlebarContainerView` paints a 1 px highlight at
            // the top even with `titlebarAppearsTransparent` +
            // `fullSizeContentView`, so hide it when chrome is fully hidden.
            // `ButtonsOnly` and `Native` keep it — the traffic lights and the
            // opaque strip live in that view. Mirrors Ghostty's
            // `HiddenTitlebarTerminalWindow.reapplyHiddenStyle`. The class is
            // AppKit-private so there's no typed wrapper — reach it by name.
            if hide_buttons
                && !native_titlebar
                && let Some(titlebar_cls) = AnyClass::get(c"NSTitlebarContainerView")
                && let Some(content_view) = nswindow.contentView()
            {
                // SAFETY: superview is `unsafe` because its reference isn't
                // retained; we use it immediately on the main thread before
                // returning, so the hierarchy cannot change underneath us.
                let theme_frame = unsafe { content_view.superview() };
                if let Some(theme_frame) = theme_frame {
                    for sub in theme_frame.subviews().iter() {
                        if sub.isKindOfClass(titlebar_cls) {
                            sub.setHidden(true);
                            break;
                        }
                    }
                }
            }
        }
    }

    let app = NSApplication::sharedApplication(mtm);
    app.activate();
    nswindow.makeKeyAndOrderFront(Some(&app));
}

#[cfg(target_os = "macos")]
fn set_standard_buttons_hidden(nswindow: &objc2_app_kit::NSWindow, hidden: bool) {
    use objc2_app_kit::NSWindowButton;

    for button in [
        NSWindowButton::CloseButton,
        NSWindowButton::MiniaturizeButton,
        NSWindowButton::ZoomButton,
    ] {
        if let Some(btn) = nswindow.standardWindowButton(button) {
            btn.setHidden(hidden);
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub fn configure_window(
    _window: &winit::window::Window,
    _decoration: seance_config::WindowDecoration,
    _titlebar_style: seance_config::TitlebarStyle,
) {
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
    tracing::info!("macOS option-as-alt -> {mode:?} (winit: {winit_mode:?})");
    window.set_option_as_alt(winit_mode);
}

#[cfg(not(target_os = "macos"))]
pub fn set_option_as_alt(_window: &winit::window::Window, _mode: seance_input::OptionAsAlt) {}

/// Translate the user's macOS-only config knob into the cross-platform
/// `seance_input::OptionAsAlt`. The config enum is always present, but
/// seance-input's encoder ignores the value outside macOS.
pub fn option_as_alt_from_config(
    cfg: seance_config::MacosOptionAsAlt,
) -> seance_input::OptionAsAlt {
    use seance_config::MacosOptionAsAlt;
    use seance_input::OptionAsAlt;
    match cfg {
        MacosOptionAsAlt::None => OptionAsAlt::None,
        MacosOptionAsAlt::Left => OptionAsAlt::Left,
        MacosOptionAsAlt::Right => OptionAsAlt::Right,
        MacosOptionAsAlt::Both => OptionAsAlt::Both,
    }
}

/// Prevent stretching during live resize on macOS by enabling
/// `CAMetalLayer.presentsWithTransaction`. Must run after the wgpu surface
/// has been created (the CAMetalLayer is attached as a sublayer then) and
/// before the first frame is presented.
#[cfg(target_os = "macos")]
pub fn configure_metal_layer(window: &winit::window::Window) {
    use objc2_app_kit::NSView;
    use objc2_quartz_core::CAMetalLayer;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::AppKit(h) = handle.as_raw() else {
        return;
    };

    // SAFETY: winit guarantees h.ns_view points to a live NSView for the
    // duration of this call; invoked synchronously from the main thread
    // right after renderer creation.
    let view: &NSView = unsafe { h.ns_view.cast::<NSView>().as_ref() };
    let Some(layer) = view.layer() else {
        return;
    };
    // SAFETY: `sublayers` is `unsafe` because its elements aren't retained;
    // we read them immediately on the main thread, so the array cannot
    // change underneath us.
    let Some(sublayers) = (unsafe { layer.sublayers() }) else {
        return;
    };
    for sub in sublayers.iter() {
        if let Ok(metal) = sub.downcast::<CAMetalLayer>() {
            metal.setPresentsWithTransaction(true);
            break;
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub fn configure_metal_layer(_window: &winit::window::Window) {}

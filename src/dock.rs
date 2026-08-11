//! macOS Dock presence for long-running foreground commands.
//!
//! A terminal CLI has no Dock icon by default. `orx up` runs a local server
//! until Ctrl-C, so — like any GUI app — it switches to a regular activation
//! policy and paints the brand logo in the Dock for as long as it runs. The OS
//! clears the icon on exit.

/// Show the orx logo in the macOS Dock for the lifetime of this process.
///
/// No-op off macOS. On macOS it must run on the main thread:
/// `MainThreadMarker::new()` returns `None` off it, and we bail rather than
/// touch AppKit from the wrong thread. (In a headless session with no window
/// server AppKit is still called but simply no icon appears — harmless.)
#[cfg(target_os = "macos")]
pub fn show_in_dock() {
    use objc2::{AnyThread, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSImage};
    use objc2_foundation::NSData;

    // 1024×1024 brand squircle, rasterized from ui/public/favicon.svg.
    const ICON_PNG: &[u8] = include_bytes!("../assets/orx-dock-icon.png");

    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);

    let data = NSData::with_bytes(ICON_PNG);
    if let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) {
        // SAFETY: `Some(&image)` is a valid non-null NSImage.
        unsafe { app.setApplicationIconImage(Some(&image)) };
    }
}

#[cfg(not(target_os = "macos"))]
pub fn show_in_dock() {}

#[cfg(target_os = "linux")]
mod native {
    use gstreamer as gst;

    fn plugin_init(_: &gst::Plugin) -> Result<(), gst::glib::BoolError> {
        let mode = std::env::var("ORX_MEDIA_FAULT").unwrap_or_default();
        eprintln!("ORX fault fixture: pid={} mode={mode}", std::process::id());
        match mode.as_str() {
            "abort" => std::process::abort(),
            "hang" => loop {
                std::thread::park();
            },
            _ => Ok(()),
        }
    }

    gst::plugin_define!(
        orxprobe,
        "Media isolation fault fixture",
        plugin_init,
        "0.1.0",
        "MIT/X11",
        "orx-media-fault-fixture",
        "OpenResearch tests",
        "https://github.com/alphaXiv/OpenResearch"
    );
}

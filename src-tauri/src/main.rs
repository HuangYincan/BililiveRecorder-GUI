fn main() {
    // On Unix the app re-runs its own binary as the sidecar guardian; that path
    // must exit before any windowing system is touched.
    #[cfg(unix)]
    if let Some(target) = bililive_recorder_gui_lib::guardian_target() {
        std::process::exit(bililive_recorder_gui_lib::run_guardian(&target));
    }
    bililive_recorder_gui_lib::run();
}

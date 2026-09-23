fn main() {
    // The app re-runs its own binary as the backend guardian. That path must
    // exit before any windowing system is touched.
    if let Some(config) = bililive_recorder_gui_lib::guardian_config() {
        std::process::exit(bililive_recorder_gui_lib::run_guardian(&config));
    }
    bililive_recorder_gui_lib::run();
}

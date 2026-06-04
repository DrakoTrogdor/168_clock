// Desktop binary entry point. The app itself lives in the library crate
// (src/lib.rs), which also exposes the Android `android_main` entry. Keeping the
// library and binary in one crate lets Android build a `cdylib` while desktop
// builds a normal executable from the same code.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // On Android the library's `android_main` is the entry point and this binary
    // is unused, so `main` is a no-op there.
    #[cfg(not(target_os = "android"))]
    weekclock::run_desktop();
}

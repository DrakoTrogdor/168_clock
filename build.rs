fn main() {
    // On Windows, embed the multi-size icon as a resource so the .exe shows it
    // in Explorer and the taskbar. No-op on other platforms.
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=assets/icon.ico");
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        if let Err(e) = res.compile() {
            // Don't fail the build if the resource compiler is unavailable.
            println!("cargo:warning=failed to embed icon resource: {e}");
        }
    }
}

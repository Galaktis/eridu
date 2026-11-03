//! Embeds the icon in the Windows executable, so the file explorer and shortcuts show it.

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    #[cfg(windows)]
    {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("assets/icon.ico");
        // A missing resource compiler only costs the file icon, not the build.
        if let Err(e) = resource.compile() {
            println!("cargo:warning=executable icon not embedded: {e}");
        }
    }
}

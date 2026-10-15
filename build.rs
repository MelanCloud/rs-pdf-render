//! Gives the Windows executable its icon, and the name Windows shows for it in
//! "Open with" and Default apps.

fn main() {
    println!("cargo::rerun-if-changed=assets/icon.ico");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winresource::WindowsResource::new();
        resource
            .set_icon("assets/icon.ico")
            .set("FileDescription", "rs-pdf-render")
            .set("ProductName", "rs-pdf-render");
        resource.compile().expect("Couldn't add the icon to the executable");
    }
}

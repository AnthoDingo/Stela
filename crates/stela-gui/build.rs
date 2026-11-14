//! Gives the Windows executable the Stela logo (`stela.ico`, made from
//! `assets/stela-icon.png`) as its file icon.
fn main() {
    println!("cargo:rerun-if-changed=stela.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("stela.ico");
        if let Err(error) = resource.compile() {
            panic!("cannot embed stela.ico: {error}");
        }
    }
}

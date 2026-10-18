//! libobs symbols resolve when OBS loads the plugin, so nothing links against libobs here.
//! Linux shared objects allow undefined symbols by default; Windows imports them through
//! `raw-dylib`; macOS needs to be told to look them up at load time.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-cdylib-link-arg=-undefined");
        println!("cargo:rustc-cdylib-link-arg=dynamic_lookup");
    }
}

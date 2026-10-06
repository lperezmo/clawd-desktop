//! Embeds Clawd's icon and the version info in the Windows exe.

fn main() {
    println!("cargo:rerun-if-changed=assets/clawd.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    let ico = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join("clawd.ico");
    // Icon group 1: the first icon resource, which Explorer and the taskbar
    // show, and which the tray loads with LoadImageW. The icon is missing
    // only while `cargo run --example make-icon` builds it for the first time.
    if ico.exists() {
        res.set_icon_with_id(&ico.to_string_lossy(), "1");
    } else {
        println!("cargo:warning=assets/clawd.ico missing; run cargo run --example make-icon");
    }
    res.set("FileDescription", "Clawd Desktop")
        .set("ProductName", "Clawd Desktop")
        .set("OriginalFilename", "clawd-desktop.exe")
        .set("LegalCopyright", "Code MIT; Clawd's art is Anthropic's (see NOTICE)");
    if let Err(e) = res.compile() {
        panic!("embedding resources: {e}");
    }
}

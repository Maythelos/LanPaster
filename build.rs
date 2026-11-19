fn main() {
    // 托盘图标由 include_bytes! 嵌进二进制；cargo 不会因为这几张 PNG
    // 变化就重编 crate，这里显式声明依赖，避免打包时用上旧图标。
    for icon in [
        "assets/tray-mac-off.png",
        "assets/tray-mac-on.png",
        "assets/tray-mac-active.png",
        "assets/tray-off-color.png",
        "assets/tray-on-color.png",
        "assets/tray-active-color.png",
    ] {
        println!("cargo:rerun-if-changed={icon}");
    }
    let config = slint_build::CompilerConfiguration::new().with_style("cupertino".into());
    slint_build::compile_with_config("ui/app.slint", config).expect("compile Slint UI");
}

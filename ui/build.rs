#[path = "../native/visual_build.rs"]
mod visual_build;

fn main() {
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let source = std::fs::read_to_string("../assets/theme.json").unwrap();
    visual_build::generate(&out, &source);
    for path in [
        "../assets/theme.json",
        "../native/visual_build.rs",
        "../native/src/mushroom.rs",
        "../native/src/appearance.rs",
    ] {
        println!("cargo:rerun-if-changed={path}");
    }
    let config = slint_build::CompilerConfiguration::new()
        .with_style("fluent-dark".into())
        .embed_resources(slint_build::EmbedResourcesKind::EmbedFiles);
    let input = if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("android") {
        let source = std::fs::read_to_string("views/app.slint").unwrap();
        let source = source.split("export component Tray inherits").next().unwrap();
        let source = source.replace("../../assets/fonts/Tiny5-Regular.ttf", &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../assets/fonts/Tiny5-Regular.ttf").to_string_lossy().replace('\\', "/"));
        // A type-compatible, never shown component keeps shared host state portable.
        let source = source + "\nexport component Tray inherits Window { in property <image> tray-icon; callback show-window; callback disconnect; callback exit; Text { text: I18n.tr(\"Mukhomor\"); } }";
        let path = out.join("mobile.slint"); std::fs::write(&path, source).unwrap(); path
    } else { std::path::PathBuf::from("views/app.slint") };
    println!("cargo:rerun-if-changed=views/app.slint");
    slint_build::compile_with_config(input, config)
        .expect("Mukhomor Slint interface must compile");
}

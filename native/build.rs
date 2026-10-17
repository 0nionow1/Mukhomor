use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

#[allow(dead_code)]
#[path = "src/mushroom.rs"]
mod mushroom;

mod visual_build;

fn icon(out: &Path, colors: &[u32]) {
    let palette = mushroom::rgb_palette(colors[0], colors[6], colors[5], colors[9], false);
    let sizes = [16usize, 20, 24, 32, 40, 48, 64, 96, 128, 256];
    let images: Vec<_> = sizes
        .into_iter()
        .map(|size| mushroom::icon_dib(size, &palette))
        .collect();
    let mut ico = vec![0, 0, 1, 0, sizes.len() as u8, 0];
    let mut offset = 6 + sizes.len() * 16;
    for (size, dib) in sizes.into_iter().zip(&images) {
        ico.extend_from_slice(&[size as u8, size as u8, 0, 0, 1, 0, 32, 0]);
        ico.extend_from_slice(&(dib.len() as u32).to_le_bytes());
        ico.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += dib.len();
    }
    for dib in images {
        ico.extend_from_slice(&dib);
    }
    fs::write(out.join("mukhomor.ico"), ico).unwrap();
}
fn main() {
    // env! records the path as a build-script dependency. A copied target cache
    // must not keep linker manifest paths from the previous checkout location.
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let out = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let source = fs::read_to_string(root.join("../assets/theme.json"))
        .expect("assets/theme.json is required");
    println!("cargo:rerun-if-changed=../assets/theme.json");
    println!("cargo:rerun-if-changed=src/mushroom.rs");
    println!("cargo:rerun-if-changed=src/mushroom_data.rs");
    println!("cargo:rerun-if-changed=visual_build.rs");
    let colors = visual_build::generate(&out, &source);
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") { return; }
    icon(&out, &colors);
    fs::write(
        out.join("icon.rc"),
        format!(
            "1 ICON \"{}\"",
            out.join("mukhomor.ico")
                .display()
                .to_string()
                .replace('\\', "\\\\")
        ),
    )
    .unwrap();
    let sdk = PathBuf::from(r"C:\Program Files (x86)\Windows Kits\10\bin");
    let mut versions: Vec<_> = fs::read_dir(&sdk)
        .expect("Windows SDK is required")
        .filter_map(Result::ok)
        .map(|v| v.path())
        .collect();
    versions.sort();
    let rc = versions
        .iter()
        .rev()
        .map(|v| v.join("x64/rc.exe"))
        .find(|v| v.is_file())
        .expect("Windows SDK rc.exe is required to embed the product icon");
    let result = Command::new(rc)
        .arg("/nologo")
        .arg("/fo")
        .arg(out.join("icon.res"))
        .arg(out.join("icon.rc"))
        .status()
        .unwrap();
    assert!(result.success(), "Windows icon resource compilation failed");
    println!("cargo:rustc-link-arg={}", out.join("icon.res").display());
    println!("cargo:rerun-if-changed=windows.manifest");
    println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
    println!(
        "cargo:rustc-link-arg=/MANIFESTINPUT:{}",
        root.join("windows.manifest").display()
    );
}

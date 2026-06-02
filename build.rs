// Branding: turn grittyicon.png into (1) a small raw-RGBA blob for the winit
// window icon and (2) a multi-size .ico embedded into the .exe resource.
// All of this runs at build time, so the runtime binary only carries the
// 64x64 window icon (~16 KB), not the full PNG.

use std::path::Path;

fn main() {
    let png = "grittyicon.png";
    println!("cargo:rerun-if-changed={png}");
    println!("cargo:rerun-if-changed=build.rs");

    let out = std::env::var("OUT_DIR").expect("OUT_DIR");
    let img = image::open(png).expect("open grittyicon.png").to_rgba8();

    // (1) Window icon: 64x64 raw RGBA.
    let win = image::imageops::resize(&img, 64, 64, image::imageops::FilterType::Lanczos3);
    std::fs::write(Path::new(&out).join("icon_rgba.bin"), win.as_raw()).expect("write rgba");

    // (2) Multi-resolution .ico for the executable.
    let mut dir = ico::IconDir::new(ico::ResourceType::Icon);
    for size in [256u32, 64, 48, 32, 16] {
        let r = image::imageops::resize(&img, size, size, image::imageops::FilterType::Lanczos3);
        let ico_img = ico::IconImage::from_rgba_data(size, size, r.into_raw());
        dir.add_entry(ico::IconDirEntry::encode(&ico_img).expect("encode ico entry"));
    }
    let ico_path = Path::new(&out).join("gritty.ico");
    let file = std::fs::File::create(&ico_path).expect("create ico");
    dir.write(file).expect("write ico");

    // (3) Embed the .ico into the exe (Explorer/taskbar icon).
    #[cfg(target_os = "windows")]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon(ico_path.to_str().expect("ico path utf8"));
        if let Err(e) = res.compile() {
            // Don't fail the whole build if the resource compiler is missing;
            // the window icon still works without the embedded exe icon.
            println!("cargo:warning=exe icon embed skipped: {e}");
        }
    }
}

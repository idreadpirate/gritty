// Branding: turn grittyicon.png into (1) a small raw-RGBA blob for the winit
// window icon and (2) a multi-size .ico embedded into the .exe resource.
// All of this runs at build time, so the runtime binary only carries the
// 64x64 window icon (~16 KB), not the full PNG.

use std::path::Path;

use image::{imageops, RgbaImage};

/// Scale `img` to fit within a `size`x`size` box preserving aspect ratio, then
/// center it on a transparent square canvas (object-fit: contain). Avoids the
/// distortion that a direct resize-to-square causes on a non-square source.
fn fit_square(img: &RgbaImage, size: u32) -> RgbaImage {
    let (w, h) = img.dimensions();
    let scale = (size as f32 / w as f32).min(size as f32 / h as f32);
    let nw = ((w as f32 * scale).round() as u32).max(1);
    let nh = ((h as f32 * scale).round() as u32).max(1);
    let resized = imageops::resize(img, nw, nh, imageops::FilterType::Lanczos3);

    let mut canvas = RgbaImage::new(size, size); // transparent
    let x = ((size - nw) / 2) as i64;
    let y = ((size - nh) / 2) as i64;
    imageops::overlay(&mut canvas, &resized, x, y);
    canvas
}

fn main() {
    let png = "grittyicon.png";
    println!("cargo:rerun-if-changed={png}");
    println!("cargo:rerun-if-changed=build.rs");

    let out = std::env::var("OUT_DIR").expect("OUT_DIR");
    let img = image::open(png).expect("open grittyicon.png").to_rgba8();

    // (1) Window icon: 64x64 raw RGBA, aspect-preserved.
    let win = fit_square(&img, 64);
    std::fs::write(Path::new(&out).join("icon_rgba.bin"), win.as_raw()).expect("write rgba");

    // (2) Multi-resolution .ico for the executable, aspect-preserved.
    let mut dir = ico::IconDir::new(ico::ResourceType::Icon);
    for size in [64u32, 48, 32, 16] {
        let sq = fit_square(&img, size);
        let ico_img = ico::IconImage::from_rgba_data(size, size, sq.into_raw());
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

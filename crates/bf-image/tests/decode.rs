//! Decodificación de PNG (generado acá y de WPT), JPEG y GIF (de WPT, si están descargados).

use std::path::PathBuf;

fn wpt(path: &str) -> Option<Vec<u8>> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/wpt")
        .join(path);
    std::fs::read(p).ok()
}

/// Un PNG RGBA de 2×1: rojo opaco y verde al 50%.
fn tiny_png() -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, 2, 1);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().unwrap();
        w.write_image_data(&[255, 0, 0, 255, 0, 255, 0, 128])
            .unwrap();
    }
    out
}

#[test]
fn png_premultiplied() {
    let img = bf_image::decode(&tiny_png(), None).unwrap();
    assert_eq!((img.width, img.height), (2, 1));
    assert_eq!(img.rgba, [255, 0, 0, 255, 0, 128, 0, 128]);
    assert_eq!(bf_image::sniff(&tiny_png()), Some("image/png"));
}

#[test]
fn wpt_images() {
    if let Some(bytes) = wpt("css/CSS2/support/60x60-green.png") {
        let img = bf_image::decode(&bytes, None).unwrap();
        assert_eq!((img.width, img.height), (60, 60));
        assert_eq!(&img.rgba[..4], &[0, 128, 0, 255]);
    }
    if let Some(bytes) = wpt("css/css-backgrounds/support/bg.jpg") {
        let img = bf_image::decode(&bytes, Some("image/jpeg")).unwrap();
        assert!(img.width > 0 && img.height > 0);
        assert_eq!(img.rgba.len(), (img.width * img.height * 4) as usize);
        assert!(img.rgba.as_chunks::<4>().0.iter().all(|p| p[3] == 255));
    }
    if let Some(bytes) = wpt("css/css-backgrounds/support/bg_flower.gif") {
        let img = bf_image::decode(&bytes, None).unwrap();
        assert!(img.width > 0);
    }
}

#[test]
fn unsupported_and_invalid() {
    assert_eq!(
        bf_image::decode(b"<svg xmlns='http://www.w3.org/2000/svg'/>", None),
        Err(bf_image::ImageError::Unsupported("image/svg+xml"))
    );
    assert!(matches!(
        bf_image::decode(
            &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0],
            None
        ),
        Err(bf_image::ImageError::Decode(_))
    ));
    let mut webp = b"RIFF\0\0\0\0WEBPVP8 ".to_vec();
    webp.extend([0; 8]);
    assert_eq!(bf_image::sniff(&webp), Some("image/webp"));
}

//! BMP, ICO y animación (GIF, APNG) contra vectores propios.
//!
//! Los vectores de `tests/vectors/` se generaron una vez con ffmpeg 9.0 a partir de `testsrc`
//! y se guardaron con su PNG de referencia (decodificado por ffmpeg):
//!
//! ```sh
//! S="-f lavfi -i testsrc=size=37x23:rate=1 -frames:v 1"
//! for f in bgr24 bgra rgb555le rgb565le pal8 monob rgb4_byte bgr8 gray; do
//!   ffmpeg $S -pix_fmt $f bmp/$f.bmp && ffmpeg -i bmp/$f.bmp -pix_fmt rgba bmp/$f.png
//! done
//! ffmpeg -f lavfi -i testsrc=size=32x32:rate=1 -frames:v 1 \
//!   -vf "format=rgba,geq=r='r(X,Y)':g='g(X,Y)':b='b(X,Y)':a='X*8'" -c:v png -f ico bmp/icon_png.ico
//! ffmpeg -f lavfi -i testsrc=size=32x32:rate=1 -frames:v 1 -pix_fmt bgra -c:v bmp -f ico bmp/icon_bmp.ico
//! ffmpeg -f lavfi -i testsrc=size=24x16:rate=10 -frames:v 3 -plays 0 -f apng anim/three.apng
//! ffmpeg -f lavfi -i testsrc=size=24x16:rate=10 -frames:v 3 -loop 0 anim/three.gif
//! ```

use std::path::PathBuf;
use std::time::Duration;

use bf_image::{Dispose, Image, decode, decode_frames, sniff};

fn vector(path: &str) -> Vec<u8> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/vectors")
        .join(path);
    std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// Diferencia máxima por canal entre dos imágenes del mismo tamaño.
fn max_diff(a: &Image, b: &Image) -> u8 {
    assert_eq!((a.width, a.height), (b.width, b.height));
    a.rgba
        .iter()
        .zip(&b.rgba)
        .map(|(x, y)| x.abs_diff(*y))
        .max()
        .unwrap_or(0)
}

#[test]
fn bmp_vectors() {
    for (name, tolerance) in [
        ("bgr24", 0),
        ("bgra", 0),
        ("pal8", 0),
        ("bgr8", 0),
        ("rgb4_byte", 0),
        ("gray", 0),
        ("monob", 0),
        // 5 bits por canal: ffmpeg y nosotros escalan a 8 bits distinto (redondeo).
        ("rgb555le", 4),
        ("rgb565le", 4),
    ] {
        let bytes = vector(&format!("bmp/{name}.bmp"));
        assert_eq!(sniff(&bytes), Some("image/bmp"), "{name}");
        let img = decode(&bytes, None).unwrap_or_else(|e| panic!("{name}: {e}"));
        let reference = decode(&vector(&format!("bmp/{name}.png")), None).unwrap();
        let d = max_diff(&img, &reference);
        assert!(d <= tolerance, "{name}: diferencia {d}");
    }
}

#[test]
fn ico_vectors() {
    for name in ["icon_png", "icon_bmp"] {
        let bytes = vector(&format!("bmp/{name}.ico"));
        assert_eq!(sniff(&bytes), Some("image/x-icon"));
        let img = decode(&bytes, None).unwrap_or_else(|e| panic!("{name}: {e}"));
        let reference = decode(&vector(&format!("bmp/{name}.png")), None).unwrap();
        assert!(max_diff(&img, &reference) <= 1, "{name}");
    }
}

/// Un BMP de 4×2 de 8 bpp con `BI_RLE8`, abajo hacia arriba, armado a mano.
#[test]
fn bmp_rle8() {
    let palette: [[u8; 4]; 3] = [[0, 0, 0, 0], [0, 0, 255, 0], [255, 0, 0, 0]]; // BGRx
    let pixels: &[u8] = &[
        2, 1, 2, 2, 0, 0, // fila de abajo: rojo rojo azul azul
        0, 4, 2, 1, 2, 1, 0, 0, // fila de arriba, tramo absoluto: azul rojo azul rojo
        0, 1, // fin
    ];
    let off = 14 + 40 + 4 * palette.len();
    let mut b = b"BM".to_vec();
    b.extend(((off + pixels.len()) as u32).to_le_bytes());
    b.extend([0; 4]);
    b.extend((off as u32).to_le_bytes());
    b.extend(40u32.to_le_bytes());
    b.extend(4i32.to_le_bytes());
    b.extend(2i32.to_le_bytes());
    b.extend(1u16.to_le_bytes());
    b.extend(8u16.to_le_bytes());
    b.extend(1u32.to_le_bytes()); // BI_RLE8
    b.extend([0; 12]);
    b.extend(3u32.to_le_bytes());
    b.extend([0; 4]);
    for p in palette {
        b.extend(p);
    }
    b.extend(pixels);
    let img = decode(&b, None).unwrap();
    let (r, bl) = ([255, 0, 0, 255], [0, 0, 255, 255]);
    let expect: Vec<u8> = [bl, r, bl, r, r, r, bl, bl].concat();
    assert_eq!(img.rgba, expect);
}

#[test]
fn truncated_bitmaps_do_not_panic() {
    for name in [
        "bmp/bgr24.bmp",
        "bmp/pal8.bmp",
        "bmp/icon_bmp.ico",
        "bmp/icon_png.ico",
    ] {
        let bytes = vector(name);
        for cut in (0..bytes.len()).step_by(7) {
            let _ = decode(&bytes[..cut], None);
        }
        let mut seed = 0x2545_F491u32;
        for _ in 0..300 {
            let mut m = bytes.clone();
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            let i = seed as usize % m.len();
            m[i] = (seed >> 16) as u8;
            let _ = decode(&m, None);
        }
    }
}

#[test]
fn animated_vectors() {
    for name in ["anim/three.apng", "anim/three.gif"] {
        let bytes = vector(name);
        let frames = decode_frames(&bytes, None).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!((frames.width, frames.height), (24, 16), "{name}");
        assert_eq!(frames.frames.len(), 3, "{name}");
        assert!(frames.is_animated());
        assert_eq!(frames.plays, None, "{name}: sin fin");
        assert!(
            frames
                .frames
                .iter()
                .all(|f| f.delay == Duration::from_millis(100)),
            "{name}"
        );
        // El primer fotograma es la imagen fija.
        assert_eq!(
            frames.frames[0].image,
            decode(&bytes, None).unwrap(),
            "{name}"
        );
        assert_ne!(frames.frames[0].image, frames.frames[1].image, "{name}");
        let at = |ms| frames.index_at(Duration::from_millis(ms));
        assert_eq!(
            [at(0), at(99), at(100), at(250), at(300), at(1010)],
            [0, 0, 1, 2, 0, 1]
        );
        assert_eq!(
            frames.frame_at(Duration::from_millis(150)),
            &frames.frames[1].image
        );
        assert_eq!(
            frames.next_change(Duration::from_millis(30)),
            Some(Duration::from_millis(70))
        );
    }
}

/// Un APNG de 2×1 con dos fotogramas de un píxel: el segundo se dispone con `PREVIOUS` y
/// compone con `OVER`; la animación se reproduce dos veces y queda en el último.
#[test]
fn apng_dispose_and_blend() {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, 2, 1);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_animated(3, 2).unwrap();
        enc.set_frame_delay(1, 2).unwrap();
        let mut w = enc.write_header().unwrap();
        // 1: rojo opaco y transparente.
        w.write_image_data(&[255, 0, 0, 255, 0, 0, 0, 0]).unwrap();
        // 2: verde al 50 % sobre el píxel de la izquierda, luego se restaura.
        w.set_frame_dimension(1, 1).unwrap();
        w.set_blend_op(png::BlendOp::Over).unwrap();
        w.set_dispose_op(png::DisposeOp::Previous).unwrap();
        w.write_image_data(&[0, 255, 0, 128]).unwrap();
        // 3: azul en el píxel de la derecha, reemplazando.
        w.set_frame_position(1, 0).unwrap();
        w.set_blend_op(png::BlendOp::Source).unwrap();
        w.set_dispose_op(png::DisposeOp::None).unwrap();
        w.write_image_data(&[0, 0, 255, 255]).unwrap();
    }
    let frames = decode_frames(&out, None).unwrap();
    assert_eq!(frames.frames.len(), 3);
    assert_eq!(frames.plays, Some(2));
    assert_eq!(frames.frames[0].image.rgba, [255, 0, 0, 255, 0, 0, 0, 0]);
    // 0.5 verde sobre rojo: (127, 128, 0, 255) premultiplicado.
    assert_eq!(frames.frames[1].image.rgba, [127, 128, 0, 255, 0, 0, 0, 0]);
    assert_eq!(frames.frames[1].dispose, Dispose::Previous);
    assert_eq!(
        frames.frames[2].image.rgba,
        [255, 0, 0, 255, 0, 0, 255, 255]
    );
    assert_eq!(frames.frames[2].region, (1, 0, 1, 1));
    // 3 fotogramas de 500 ms, dos pasadas: desde 2.5 s queda el último.
    let at = |ms| frames.index_at(Duration::from_millis(ms));
    assert_eq!([at(0), at(1600), at(2600), at(60_000)], [0, 0, 2, 2]);
    assert_eq!(frames.next_change(Duration::from_millis(2600)), None);
    assert_eq!(
        frames.next_change(Duration::from_millis(2400)),
        Some(Duration::from_millis(100))
    );
}

#[test]
fn still_images_have_one_frame() {
    let bytes = vector("bmp/bgr24.png");
    let frames = decode_frames(&bytes, None).unwrap();
    assert!(!frames.is_animated());
    assert_eq!(frames.next_change(Duration::ZERO), None);
    assert_eq!(
        frames.frame_at(Duration::from_secs(5)),
        &decode(&bytes, None).unwrap()
    );
}

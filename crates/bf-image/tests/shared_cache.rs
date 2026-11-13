//! E2: [`bf_image::decode_frames_shared`] decodifica cada imagen una vez por proceso. Un solo
//! test en este binario: el contador es global.

use std::sync::Arc;

#[test]
fn same_bytes_decode_once() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/vectors");
    let png = std::fs::read(dir.join("bmp/icon_png.png")).unwrap();
    let gray = std::fs::read(dir.join("jpeg/gray-17x9.jpg")).unwrap();
    let start = bf_image::decode_count();
    let a = bf_image::decode_frames_shared(&png, None).unwrap();
    let b = bf_image::decode_frames_shared(&png, Some("image/png")).unwrap();
    let c = bf_image::decode_frames_shared(&png, None).unwrap();
    // El tipo es parte de la clave: `None` y `image/png` son dos entradas; la tercera llamada
    // sale de la caché, con el mismo resultado compartido.
    assert_eq!(bf_image::decode_count() - start, 2);
    assert!(Arc::ptr_eq(&a, &c));
    assert_eq!((a.width, a.height), (b.width, b.height));
    let j = bf_image::decode_frames_shared(&gray, None).unwrap();
    assert_eq!((j.width, j.height), (17, 9));
    assert_eq!(bf_image::decode_count() - start, 3);
    // Los errores también se guardan: no se vuelve a intentar.
    let bad = b"no es una imagen";
    assert!(bf_image::decode_frames_shared(bad, None).is_err());
    assert!(bf_image::decode_frames_shared(bad, None).is_err());
    assert_eq!(bf_image::decode_count() - start, 4);
}

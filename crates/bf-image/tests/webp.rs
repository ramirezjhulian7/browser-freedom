//! Decodificador WebP propio contra referencias de ffmpeg (que codifica con libwebp y
//! decodifica con su decodificador nativo), más un fuzz determinista de robustez.
//!
//! Los vectores de `tests/vectors/webp/` se generaron una vez con ffmpeg 9.0.1 (`F="ffmpeg
//! -hide_banner -loglevel error -y"`):
//!
//! ```sh
//! # Sin pérdida (VP8L): niveles de compresión 0..6 (predictor, color, resta de verde, caché,
//! # meta-prefijos), paletas de 2, 4, 16 y más colores (empaquetado de 8, 4, 2 y 1 píxeles)
//! # y alfa.
//! $F -f lavfi -i "gradients=s=37x23:seed=1" -frames:v 1 -pix_fmt bgra -c:v libwebp -lossless 1 -compression_level 0 ll_grad_c0.webp
//! $F -f lavfi -i "testsrc=s=64x64" -frames:v 1 -pix_fmt bgra -c:v libwebp -lossless 1 -compression_level 1 ll_testsrc_c1.webp
//! $F -f lavfi -i "mandelbrot=s=129x97" -frames:v 1 -pix_fmt bgra -c:v libwebp -lossless 1 -compression_level 2 ll_mandel_c2.webp
//! $F -f lavfi -i "smptebars=s=129x97" -frames:v 1 -pix_fmt bgra -c:v libwebp -lossless 1 -compression_level 3 ll_smpte_c3.webp
//! $F -f lavfi -i "color=c=0x806040:s=40x33,format=gbrp,noise=alls=90:allf=u" -frames:v 1 -pix_fmt bgra -c:v libwebp -lossless 1 -compression_level 4 ll_noise_c4.webp
//! $F -f lavfi -i "testsrc2=s=99x61" -frames:v 1 -pix_fmt bgra -c:v libwebp -lossless 1 -compression_level 5 ll_testsrc2_c5.webp
//! $F -f lavfi -i "gradients=s=129x97:seed=7:type=radial" -frames:v 1 -pix_fmt bgra -c:v libwebp -lossless 1 -compression_level 6 ll_radial_c6.webp
//! $F -f lavfi -i "color=c=0x806040:s=64x48,format=gbrp,noise=alls=20:allf=t,gblur=sigma=2" -frames:v 1 -pix_fmt bgra -c:v libwebp -lossless 1 -compression_level 0 ll_blur_c0.webp
//! $F -f lavfi -i "nullsrc=s=37x23,format=rgba,geq=r='255*gt(mod(X+2*Y,7),3)':g='40':b='255*gt(mod(X+2*Y,7),3)':a='255'" -frames:v 1 -pix_fmt bgra -c:v libwebp -lossless 1 -compression_level 6 ll_pal2.webp
//! $F -f lavfi -i "nullsrc=s=37x23,format=rgba,geq=r='85*mod(X+Y,4)':g='255-85*mod(X+Y,4)':b='100':a='255'" -frames:v 1 -pix_fmt bgra -c:v libwebp -lossless 1 -compression_level 6 ll_pal4.webp
//! $F -f lavfi -i "nullsrc=s=45x29,format=rgba,geq=r='17*mod(floor(X/3)+Y,16)':g='255-17*mod(floor(X/3)+Y,16)':b='17*mod(X,2)*mod(floor(X/3)+Y,16)':a='255'" -frames:v 1 -pix_fmt bgra -c:v libwebp -lossless 1 -compression_level 6 ll_pal16.webp
//! $F -f lavfi -i "testsrc=s=50x40,format=rgba,geq=r='r(X,Y)':g='g(X,Y)':b='b(X,Y)':a='255*X/W'" -frames:v 1 -pix_fmt bgra -c:v libwebp -lossless 1 -compression_level 4 ll_alpha.webp
//! for f in ll_*.webp; do $F -i "$f" -pix_fmt rgba "${f%.webp}.png"; done
//!
//! # Con pérdida (VP8), con calidades y presets variados (segmentos, nitidez, B_PRED...);
//! # con alfa (VP8X + ALPH):
//! $F -f lavfi -i "testsrc=s=37x23" -frames:v 1 -pix_fmt yuv420p -c:v libwebp -quality 10 ly_testsrc_q10.webp
//! $F -f lavfi -i "gradients=s=64x64:seed=3" -frames:v 1 -pix_fmt yuv420p -c:v libwebp -quality 50 -preset picture ly_grad_q50.webp
//! $F -f lavfi -i "smptebars=s=129x97" -frames:v 1 -pix_fmt yuv420p -c:v libwebp -quality 90 -preset drawing ly_smpte_q90.webp
//! $F -f lavfi -i "mandelbrot=s=129x97" -frames:v 1 -pix_fmt yuv420p -c:v libwebp -quality 60 -preset photo ly_mandel_photo.webp
//! $F -f lavfi -i "testsrc2=s=99x61" -frames:v 1 -pix_fmt yuv420p -c:v libwebp -quality 40 -preset icon ly_testsrc2_icon.webp
//! $F -f lavfi -i "nullsrc=s=48x48,geq=lum='random(1)*255':cb=128:cr=128" -frames:v 1 -pix_fmt yuv420p -c:v libwebp -quality 75 -preset text ly_noise_text.webp
//! $F -f lavfi -i "testsrc=s=50x40,format=rgba,geq=r='r(X,Y)':g='g(X,Y)':b='b(X,Y)':a='255*X/W'" -frames:v 1 -pix_fmt yuva420p -c:v libwebp -quality 75 ly_alpha_q75.webp
//! $F -f lavfi -i "smptebars=s=61x47,format=rgba,geq=r='r(X,Y)':g='g(X,Y)':b='b(X,Y)':a='if(lt(hypot(X-30,Y-23),18),255,40*mod(X,3))'" -frames:v 1 -pix_fmt yuva420p -c:v libwebp -quality 30 -compression_level 6 ly_alpha_q30.webp
//! # Derivados (filtro simple; deltas de segmento y de filtro por modo/referencia), que
//! # libwebp no produce desde ffmpeg: recodifican la partición 0 de otro vector, ver
//! # `generate_derived_vectors` en `src/webp/vp8.rs` (`ly_simple_filter`, `ly_deltas`).
//!
//! # Animados (3 fotogramas); la referencia es el primero:
//! $F -f lavfi -i "testsrc=s=40x30:rate=5" -frames:v 3 -pix_fmt bgra -c:v libwebp_anim -lossless 1 -loop 0 anim_ll.webp
//! $F -f lavfi -i "testsrc=s=40x30:rate=5" -frames:v 1 -pix_fmt rgba anim_ll.png
//! $F -f lavfi -i "testsrc=s=40x30:rate=5" -frames:v 3 -pix_fmt yuv420p -c:v libwebp_anim -quality 50 -loop 0 anim_ly.webp
//! $F -i anim_ly.webp -frames:v 1 -vf scale=flags=bilinear+full_chroma_int+accurate_rnd -pix_fmt rgba anim_ly.png
//!
//! # Referencias con pérdida: los planos YUV 4:2:0 exactos (la prueba unitaria de
//! # `src/webp/vp8.rs` exige que coincidan muestra a muestra) y un RGBA con croma bilineal
//! # centrada, como el upsampling "fancy" de libwebp. La conversión de swscale desalinea la
//! # croma en tamaños impares, así que antes se rellena la luma hasta tamaño par repitiendo
//! # la última columna y fila (la croma ya cubre el tamaño par), y luego se recorta:
//! for f in ly_*.webp; do
//!   b="${f%.webp}"
//!   $F -i "$f" -f rawvideo -pix_fmt yuv420p "$b.yuv"
//!   # W×H: tamaño de la imagen; EW×EH: redondeado a par; pad.yuv: $b.yuv rellenado (script
//!   # de Python de 8 líneas, solo bytes: cada fila de luma + su último byte repetido EW−W
//!   # veces, la última fila repetida EH−H veces, y los planos U y V tal cual).
//!   $F -f rawvideo -pix_fmt yuv420p -s ${EW}x${EH} -i pad.yuv -i "$f" -filter_complex \
//!     "[0]scale=flags=bilinear+full_chroma_int+accurate_rnd,format=rgba,crop=$W:$H:0:0[c];[1]format=rgba,alphaextract[a];[c][a]alphamerge" "$b.png"
//! done
//! ```

use std::path::{Path, PathBuf};

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/vectors/webp")
}

fn vectors() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "webp"))
        .collect();
    v.sort();
    assert!(v.len() >= 20, "faltan vectores");
    v
}

fn name(p: &Path) -> String {
    p.file_stem().unwrap().to_string_lossy().into_owned()
}

fn reference(p: &Path) -> bf_image::Image {
    let png = std::fs::read(p.with_extension("png")).unwrap();
    bf_image::decode(&png, None).unwrap()
}

fn le32(b: &[u8], o: usize) -> usize {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) as usize
}

/// Chunks de primer nivel: `(fourcc, inicio del contenido, tamaño)`.
fn chunks(bytes: &[u8]) -> Vec<([u8; 4], usize, usize)> {
    let mut out = Vec::new();
    let mut pos = 12;
    while pos + 8 <= bytes.len() {
        let size = le32(bytes, pos + 4);
        let fourcc = [bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]];
        out.push((fourcc, pos + 8, size));
        pos += 8 + size + size % 2;
    }
    out
}

fn chunk(fourcc: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut out = fourcc.to_vec();
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(data);
    if data.len() % 2 == 1 {
        out.push(0);
    }
    out
}

fn riff(body: &[u8]) -> Vec<u8> {
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&(4 + body.len() as u32).to_le_bytes());
    out.extend_from_slice(b"WEBP");
    out.extend_from_slice(body);
    out
}

#[test]
fn sniff_is_webp() {
    for p in vectors() {
        let bytes = std::fs::read(&p).unwrap();
        assert_eq!(bf_image::sniff(&bytes), Some("image/webp"), "{}", name(&p));
    }
}

#[test]
fn lossless_is_exact() {
    for p in vectors() {
        let n = name(&p);
        if !n.starts_with("ll_") && n != "anim_ll" {
            continue;
        }
        let img = bf_image::decode(&std::fs::read(&p).unwrap(), None)
            .unwrap_or_else(|e| panic!("{n}: {e}"));
        let r = reference(&p);
        assert_eq!((img.width, img.height), (r.width, r.height), "{n}");
        let diffs = img.rgba.iter().zip(&r.rgba).filter(|(a, b)| a != b).count();
        assert_eq!(diffs, 0, "{n}: {diffs} bytes distintos");
    }
}

/// Con pérdida: los planos YUV son exactos (prueba unitaria de `vp8.rs`); el RGB difiere de la
/// referencia solo por el redondeo de la interpolación de croma y de la conversión.
#[test]
fn lossy_is_close() {
    for p in vectors() {
        let n = name(&p);
        if !n.starts_with("ly_") && n != "anim_ly" {
            continue;
        }
        let img = bf_image::decode(&std::fs::read(&p).unwrap(), None)
            .unwrap_or_else(|e| panic!("{n}: {e}"));
        let r = reference(&p);
        assert_eq!((img.width, img.height), (r.width, r.height), "{n}");
        let (mut sum, mut max, mut alpha_max) = (0u64, 0u8, 0u8);
        for (a, b) in img
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .zip(r.rgba.as_chunks::<4>().0)
        {
            for c in 0..3 {
                let d = a[c].abs_diff(b[c]);
                sum += u64::from(d);
                max = max.max(d);
            }
            alpha_max = alpha_max.max(a[3].abs_diff(b[3]));
        }
        let mean = sum as f64 / (img.rgba.len() / 4 * 3) as f64;
        println!("{n}: error medio {mean:.3}, máximo {max}, alfa máximo {alpha_max}");
        assert!(mean < 1.0, "{n}: error medio {mean}");
        assert!(max <= 2, "{n}: error máximo {max}");
        assert_eq!(alpha_max, 0, "{n}: el alfa debe ser exacto");
    }
}

#[test]
fn animated_gives_first_frame_on_canvas() {
    let bytes = std::fs::read(dir().join("anim_ll.webp")).unwrap();
    let img = bf_image::decode(&bytes, Some("image/webp")).unwrap();
    assert_eq!((img.width, img.height), (40, 30));
}

/// Filtro predictivo del canal alfa hacia adelante (el inverso del que deshace el
/// decodificador): `filter` 1 = horizontal, 2 = vertical, 3 = gradiente.
fn filter_alpha(a: &[u8], w: usize, h: usize, filter: u8) -> Vec<u8> {
    let mut out = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let pred = match (x, y) {
                (0, 0) => 0,
                (_, 0) => a[i - 1],
                (0, _) => a[i - w],
                _ => match filter {
                    1 => a[i - 1],
                    2 => a[i - w],
                    _ => {
                        let g = i32::from(a[i - 1]) + i32::from(a[i - w]) - i32::from(a[i - w - 1]);
                        g.clamp(0, 255) as u8
                    }
                },
            };
            out[i] = a[i].wrapping_sub(pred);
        }
    }
    out
}

/// `ALPH` sin compresión y con cada filtro: se rearma `ly_alpha_q75.webp` con el alfa de su
/// referencia filtrado y debe decodificar igual que el original (que usa VP8L sin filtro).
#[test]
fn alpha_filters_and_raw() {
    let bytes = std::fs::read(dir().join("ly_alpha_q75.webp")).unwrap();
    let original = bf_image::decode(&bytes, None).unwrap();
    let (w, h) = (original.width as usize, original.height as usize);
    let alpha: Vec<u8> = reference(&dir().join("ly_alpha_q75.webp"))
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| p[3])
        .collect();
    let cs = chunks(&bytes);
    let get = |fourcc: &[u8; 4]| {
        let &(_, start, size) = cs.iter().find(|c| &c.0 == fourcc).unwrap();
        &bytes[start..start + size]
    };
    for filter in 0..4u8 {
        let mut alph = vec![filter << 2];
        alph.extend(if filter == 0 {
            alpha.clone()
        } else {
            filter_alpha(&alpha, w, h, filter)
        });
        let mut body = chunk(b"VP8X", get(b"VP8X"));
        body.extend(chunk(b"ALPH", &alph));
        body.extend(chunk(b"VP8 ", get(b"VP8 ")));
        let img = bf_image::decode(&riff(&body), None).unwrap();
        assert_eq!(img, original, "filtro {filter}");
    }
}

/// Generador congruencial lineal fijo (Numerical Recipes).
struct Lcg(u32);

impl Lcg {
    fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(1664525).wrapping_add(1013904223);
        self.0
    }
}

fn decode_no_panic(bytes: &[u8], what: &str) -> Result<bf_image::Image, bf_image::ImageError> {
    match std::panic::catch_unwind(|| bf_image::decode(bytes, Some("image/webp"))) {
        Ok(r) => {
            if let Ok(img) = &r {
                assert_eq!(
                    img.rgba.len(),
                    (img.width * img.height * 4) as usize,
                    "{what}"
                );
            }
            r
        }
        Err(_) => panic!("pánico: {what}"),
    }
}

#[test]
fn truncated_and_corrupt_never_panic() {
    let mut rng = Lcg(0x5eed);
    for p in vectors() {
        let n = name(&p);
        let bytes = std::fs::read(&p).unwrap();
        let cs = chunks(&bytes);
        for i in 0..20 {
            let cut = bytes.len() * i / 20;
            // Tal cual: el RIFF declara más bytes de los que hay.
            let r = decode_no_panic(&bytes[..cut], &format!("{n} truncado en {cut}"));
            assert!(r.is_err(), "{n}: truncado en {cut} decodificó");
            // Con los tamaños del RIFF y del chunk cortado corregidos, para que el corte
            // llegue a los flujos VP8/VP8L.
            if cut < 12 {
                continue;
            }
            let mut t = bytes[..cut].to_vec();
            t[4..8].copy_from_slice(&(cut as u32 - 8).to_le_bytes());
            if let Some(&(_, start, _)) = cs.iter().find(|c| c.1 <= cut && cut < c.1 + c.2) {
                t[start - 4..start].copy_from_slice(&((cut - start) as u32).to_le_bytes());
            }
            let _ = decode_no_panic(&t, &format!("{n} recortado en {cut}"));
        }
        // ~200 mutaciones: de 1 a 4 bytes cambiados por archivo mutado.
        for k in 0..200 {
            let mut m = bytes.clone();
            for _ in 0..=rng.next() % 4 {
                let i = rng.next() as usize % m.len();
                m[i] = rng.next() as u8;
            }
            let _ = decode_no_panic(&m, &format!("{n} mutación {k}"));
        }
    }
}

#[test]
fn garbage_is_error() {
    assert!(bf_image::decode(b"RIFF\x04\0\0\0WEBP", Some("image/webp")).is_err());
    assert!(bf_image::decode(b"RIFF\x0c\0\0\0WEBPVP8 \0\0\0\0", None).is_err());
    assert!(bf_image::decode(b"RIFF\x0c\0\0\0WEBPVP8L\0\0\0\0", None).is_err());
    assert!(bf_image::decode(b"RIFF\x0c\0\0\0WEBPVP8X\0\0\0\0", None).is_err());
}

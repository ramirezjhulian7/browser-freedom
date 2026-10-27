//! PNG (RFC 2083) y APNG con el códec `png`.

use std::io::Cursor;
use std::time::Duration;

use crate::anim::{Blend, Compositor, Dispose, ImageFrames};
use crate::{Image, ImageError, check_size, premultiply};

fn err(e: ::png::DecodingError) -> ImageError {
    ImageError::Decode(e.to_string())
}

/// Convierte la salida del lector (ya expandida a 8 bits) a RGBA sin premultiplicar.
fn to_rgba(data: &[u8], color: ::png::ColorType, n: usize) -> Result<Vec<u8>, ImageError> {
    let mut rgba = Vec::with_capacity(n * 4);
    match color {
        ::png::ColorType::Rgba => rgba.extend_from_slice(data),
        ::png::ColorType::Rgb => {
            for p in data.as_chunks::<3>().0 {
                rgba.extend_from_slice(&[p[0], p[1], p[2], 255]);
            }
        }
        ::png::ColorType::GrayscaleAlpha => {
            for p in data.as_chunks::<2>().0 {
                rgba.extend_from_slice(&[p[0], p[0], p[0], p[1]]);
            }
        }
        ::png::ColorType::Grayscale => {
            for &g in data {
                rgba.extend_from_slice(&[g, g, g, 255]);
            }
        }
        ::png::ColorType::Indexed => return Err(crate::invalid("paleta sin expandir")),
    }
    rgba.truncate(n * 4);
    rgba.resize(n * 4, 0);
    Ok(rgba)
}

fn reader(bytes: &[u8]) -> Result<::png::Reader<Cursor<&[u8]>>, ImageError> {
    let mut decoder = ::png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(::png::Transformations::EXPAND | ::png::Transformations::STRIP_16);
    let reader = decoder.read_info().map_err(err)?;
    check_size(reader.info().width, reader.info().height)?;
    Ok(reader)
}

/// La imagen por defecto (`IDAT`).
pub fn decode(bytes: &[u8]) -> Result<Image, ImageError> {
    let mut reader = reader(bytes)?;
    let (w, h) = (reader.info().width, reader.info().height);
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).map_err(err)?;
    let mut rgba = to_rgba(&buf[..info.buffer_size()], info.color_type, (w * h) as usize)?;
    premultiply(&mut rgba);
    Ok(Image {
        width: w,
        height: h,
        rgba,
    })
}

/// Los fotogramas de un APNG (o la imagen sola si no tiene `acTL`). Si el `IDAT` no lleva
/// `fcTL` delante, no es parte de la animación y se salta.
pub fn decode_frames(bytes: &[u8]) -> Result<ImageFrames, ImageError> {
    let mut reader = reader(bytes)?;
    let Some(actl) = reader.info().animation_control else {
        return decode(bytes).map(ImageFrames::still);
    };
    let (w, h) = (reader.info().width, reader.info().height);
    let mut skip_default = reader.info().frame_control.is_none();
    let mut comp = Compositor::new(w, h);
    let mut buf = vec![0; reader.output_buffer_size()];
    for _ in 0..=actl.num_frames {
        let info = match reader.next_frame(&mut buf) {
            Ok(info) => info,
            // Un fotograma dañado corta la animación, pero lo ya decodificado vale.
            Err(_) if !comp.frames.is_empty() => break,
            Err(e) => return Err(err(e)),
        };
        if std::mem::take(&mut skip_default) {
            continue;
        }
        let Some(fc) = reader.info().frame_control else {
            break;
        };
        let rgba = to_rgba(
            &buf[..info.buffer_size()],
            info.color_type,
            (info.width * info.height) as usize,
        )?;
        let den = if fc.delay_den == 0 {
            100
        } else {
            fc.delay_den
        };
        let delay = Duration::from_secs_f64(f64::from(fc.delay_num) / f64::from(den));
        let dispose = match fc.dispose_op {
            ::png::DisposeOp::None => Dispose::None,
            ::png::DisposeOp::Background => Dispose::Background,
            ::png::DisposeOp::Previous => Dispose::Previous,
        };
        let blend = match fc.blend_op {
            ::png::BlendOp::Source => Blend::Source,
            ::png::BlendOp::Over => Blend::Over,
        };
        let region = (fc.x_offset, fc.y_offset, info.width, info.height);
        if !comp.push(region, rgba, delay, dispose, blend) {
            break;
        }
        if comp.frames.len() as u32 >= actl.num_frames {
            break;
        }
    }
    if comp.frames.is_empty() {
        return decode(bytes).map(ImageFrames::still);
    }
    let plays = (actl.num_plays != 0).then_some(actl.num_plays);
    Ok(comp.finish(plays))
}

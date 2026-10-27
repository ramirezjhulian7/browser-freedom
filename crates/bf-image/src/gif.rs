//! GIF89a con el códec `gif`: el primer cuadro, o todos compuestos sobre el lienzo lógico.

use std::io::Cursor;
use std::time::Duration;

use crate::anim::{Blend, Compositor, Dispose, ImageFrames};
use crate::{Image, ImageError, check_size};

fn err(e: ::gif::DecodingError) -> ImageError {
    ImageError::Decode(e.to_string())
}

/// Decodifica hasta `max_frames` cuadros.
fn frames(bytes: &[u8], max_frames: usize) -> Result<ImageFrames, ImageError> {
    let mut options = ::gif::DecodeOptions::new();
    options.set_color_output(::gif::ColorOutput::RGBA);
    let mut decoder = options.read_info(Cursor::new(bytes)).map_err(err)?;
    let (w, h) = (u32::from(decoder.width()), u32::from(decoder.height()));
    check_size(w, h)?;
    let mut comp = Compositor::new(w, h);
    while comp.frames.len() < max_frames {
        let frame = match decoder.read_next_frame() {
            Ok(Some(frame)) => frame,
            Ok(None) => break,
            // Un cuadro truncado corta la animación (los navegadores muestran lo que hay).
            Err(_) if !comp.frames.is_empty() => break,
            Err(e) => return Err(err(e)),
        };
        let dispose = match frame.dispose {
            ::gif::DisposalMethod::Background => Dispose::Background,
            ::gif::DisposalMethod::Previous => Dispose::Previous,
            _ => Dispose::None,
        };
        let region = (
            u32::from(frame.left),
            u32::from(frame.top),
            u32::from(frame.width),
            u32::from(frame.height),
        );
        let delay = Duration::from_millis(u64::from(frame.delay) * 10);
        let rgba = frame.buffer.to_vec();
        if !comp.push(region, rgba, delay, dispose, Blend::Over) {
            break;
        }
    }
    if comp.frames.is_empty() {
        // Sin cuadros: el lienzo vacío.
        comp.push((0, 0, 0, 0), Vec::new(), Duration::ZERO, Dispose::None, Blend::Over);
    }
    // NETSCAPE2.0: el contador es de repeticiones (0 = sin fin); sin la extensión, una vez.
    let plays = match decoder.repeat() {
        ::gif::Repeat::Infinite => None,
        ::gif::Repeat::Finite(n) => Some(u32::from(n) + 1),
    };
    Ok(comp.finish(plays))
}

pub fn decode(bytes: &[u8]) -> Result<Image, ImageError> {
    let mut f = frames(bytes, 1)?;
    Ok(f.frames.swap_remove(0).image)
}

pub fn decode_frames(bytes: &[u8]) -> Result<ImageFrames, ImageError> {
    frames(bytes, usize::MAX)
}

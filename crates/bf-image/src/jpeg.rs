//! JPEG con el códec `zune-jpeg`.

use crate::{Image, ImageError, check_size};

pub fn decode(bytes: &[u8]) -> Result<Image, ImageError> {
    use zune_jpeg::zune_core::colorspace::ColorSpace;
    use zune_jpeg::zune_core::options::DecoderOptions;
    let options = DecoderOptions::default()
        .jpeg_set_out_colorspace(ColorSpace::RGBA)
        .set_max_width(1 << 14)
        .set_max_height(1 << 14);
    let mut decoder = zune_jpeg::JpegDecoder::new_with_options(bytes, options);
    let rgba = decoder
        .decode()
        .map_err(|e| ImageError::Decode(format!("{e:?}")))?;
    let info = decoder
        .info()
        .ok_or_else(|| crate::invalid("sin cabecera"))?;
    let (w, h) = (u32::from(info.width), u32::from(info.height));
    check_size(w, h)?;
    if rgba.len() != (w * h * 4) as usize {
        return Err(crate::invalid("tamaño inesperado"));
    }
    Ok(Image {
        width: w,
        height: h,
        rgba,
    })
}

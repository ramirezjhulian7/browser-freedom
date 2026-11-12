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
    let pixels = decoder
        .decode()
        .map_err(|e| ImageError::Decode(format!("{e:?}")))?;
    let info = decoder
        .info()
        .ok_or_else(|| crate::invalid("sin cabecera"))?;
    let (w, h) = (u32::from(info.width), u32::from(info.height));
    check_size(w, h)?;
    // Un JPEG de un solo componente (escala de grises, ITU T.81 §A.2) sale como luminancia
    // aunque se pida RGBA: `zune-jpeg` cambia el espacio de salida al leer el SOF. Se expande
    // según el espacio que de verdad entregó.
    let out = decoder
        .get_output_colorspace()
        .unwrap_or(ColorSpace::RGBA);
    let rgba = match out {
        ColorSpace::RGBA => pixels,
        ColorSpace::Luma => pixels.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        ColorSpace::LumaA => pixels
            .chunks_exact(2)
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        ColorSpace::RGB => pixels
            .chunks_exact(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        _ => return Err(crate::invalid("espacio de color de salida inesperado")),
    };
    if rgba.len() != (w * h * 4) as usize {
        return Err(crate::invalid("tamaño inesperado"));
    }
    Ok(Image {
        width: w,
        height: h,
        rgba,
    })
}

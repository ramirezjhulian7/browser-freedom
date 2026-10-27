//! WebP (RIFF): VP8L sin pérdida, VP8 con pérdida y `ALPH`.

use crate::{Image, ImageError};

pub fn decode(_bytes: &[u8]) -> Result<Image, ImageError> {
    Err(ImageError::Unsupported("image/webp"))
}

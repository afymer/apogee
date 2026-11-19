use std::io::Cursor;
use tracing::info;

#[derive(Debug, thiserror::Error)]
pub enum RawError {
    #[error("Empty RAW image buffer")]
    EmptyBuffer,
    #[error("Failed to decode RAW image: {0}")]
    DecodeFailed(String),
    #[error("Invalid image dimensions: {0}x{1}")]
    InvalidDimensions(u32, u32),
}

/// Pretty similar to rawloader::RawImage
#[derive(Clone, Debug)]
pub struct RawImage {
    pub make: String,
    pub model: String,
    pub width: u32,
    pub height: u32,
    pub raw_pixels: Vec<u16>,
    pub cfa_pattern: [u32; 4],
    pub black_levels: [f32; 4],
    pub white_levels: [f32; 4],
    pub wb_coeffs: [f32; 4],
}

impl RawImage {
    pub fn decode_from_memory(buffer: &[u8]) -> Result<Self, RawError> {
        if buffer.is_empty() {
            return Err(RawError::EmptyBuffer);
        }

        let mut cursor = Cursor::new(buffer);
        let raw_image =
            rawloader::decode(&mut cursor).map_err(|e| RawError::DecodeFailed(e.to_string()))?;

        let width = raw_image.width as u32;
        let height = raw_image.height as u32;

        if width == 0 || height == 0 {
            return Err(RawError::InvalidDimensions(width, height));
        }
        if raw_image.cpp != 1 {
            return Err(RawError::DecodeFailed(format!(
                "Unsupported multi-component RAW image (cpp={}, expected 1)",
                raw_image.cpp
            )));
        }

        let raw_pixels = match raw_image.data {
            rawloader::RawImageData::Integer(v) => v,
            rawloader::RawImageData::Float(v) => v
                .into_iter()
                .map(|f| (f.clamp(0.0, 1.0) * 65535.0).round() as u16)
                .collect(),
        };

        let expected_pixels = (width as usize).checked_mul(height as usize);
        if expected_pixels != Some(raw_pixels.len()) {
            return Err(RawError::InvalidDimensions(width, height));
        }

        let cfa_pattern = [
            raw_image.cfa.color_at(0, 0) as u32,
            raw_image.cfa.color_at(0, 1) as u32,
            raw_image.cfa.color_at(1, 0) as u32,
            raw_image.cfa.color_at(1, 1) as u32,
        ];

        let black_levels = [
            raw_image.blacklevels[0] as f32,
            raw_image.blacklevels[1] as f32,
            raw_image.blacklevels[2] as f32,
            raw_image.blacklevels[3] as f32,
        ];

        let mut white_levels = [
            raw_image.whitelevels[0] as f32,
            raw_image.whitelevels[1] as f32,
            raw_image.whitelevels[2] as f32,
            raw_image.whitelevels[3] as f32,
        ];
        if white_levels.iter().all(|&w| w == 0.0) {
            white_levels = [16383.0, 16383.0, 16383.0, 16383.0];
        }

        let mut wb_coeffs = [
            if raw_image.wb_coeffs[0].is_finite() {
                raw_image.wb_coeffs[0]
            } else {
                1.0
            },
            if raw_image.wb_coeffs[1].is_finite() {
                raw_image.wb_coeffs[1]
            } else {
                1.0
            },
            if raw_image.wb_coeffs[2].is_finite() {
                raw_image.wb_coeffs[2]
            } else {
                1.0
            },
            if raw_image.wb_coeffs[3].is_finite() {
                raw_image.wb_coeffs[3]
            } else {
                1.0
            },
        ];
        if wb_coeffs[1] > 0.0 {
            let g = wb_coeffs[1];
            wb_coeffs[0] /= g;
            wb_coeffs[1] = 1.0;
            wb_coeffs[2] /= g;
            wb_coeffs[3] /= g;
        }

        info!(
            make = %raw_image.make,
            model = %raw_image.model,
            dimensions = %format!("{width}x{height}"),
            cfa = %raw_image.cfa.to_string(),
            "Decoded RAW from memory successfully"
        );

        Ok(Self {
            make: raw_image.make,
            model: raw_image.model,
            width,
            height,
            raw_pixels,
            cfa_pattern,
            black_levels,
            white_levels,
            wb_coeffs,
        })
    }
}

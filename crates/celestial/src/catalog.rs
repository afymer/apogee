use bytemuck::Zeroable;
use std::io::{Read, Write};

pub const STAR_CATALOG_MAGIC: [u8; 4] = *b"APST";
pub const STAR_CATALOG_VERSION: u32 = 1;

/// 16-byte packed star record representing a star in ICRS coordinates
/// with photometric attributes. Designed for zero-copy GPU upload.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct StarRecord {
    /// Unit vector direction in ICRS (International Celestial Reference System)
    pub pos: [f32; 3],
    /// Apparent magnitude * 100 (e.g. -1.46 for Sirius -> -146, 5.24 -> 524)
    pub mag_scaled: i16,
    /// Gaia BP - RP color index * 100 (e.g. 0.82 -> 82)
    pub color_scaled: i16,
}

impl StarRecord {
    pub fn new(pos: [f32; 3], mag: f32, color_index: f32) -> Self {
        Self {
            pos,
            mag_scaled: (mag * 100.0)
                .round()
                .clamp(i16::MIN as f32, i16::MAX as f32) as i16,
            color_scaled: (color_index * 100.0)
                .round()
                .clamp(i16::MIN as f32, i16::MAX as f32) as i16,
        }
    }

    pub fn from_icrs_spherical(ra_rad: f64, dec_rad: f64, mag: f32, color_index: f32) -> Self {
        let (sin_ra, cos_ra) = ra_rad.sin_cos();
        let (sin_dec, cos_dec) = dec_rad.sin_cos();

        let x = (cos_dec * cos_ra) as f32;
        let y = (cos_dec * sin_ra) as f32;
        let z = sin_dec as f32;

        Self::new([x, y, z], mag, color_index)
    }

    #[inline]
    pub fn mag(&self) -> f32 {
        self.mag_scaled as f32 / 100.0
    }

    #[inline]
    pub fn color_index(&self) -> f32 {
        self.color_scaled as f32 / 100.0
    }
}

/// 32-byte header at the start of an Apogee Star Catalog file
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct StarCatalogHeader {
    pub magic: [u8; 4],
    pub version: u32,
    pub star_count: u32,
    pub min_mag: f32,
    pub max_mag: f32,
    pub padding: [u8; 12],
}

impl StarCatalogHeader {
    pub fn new(star_count: u32, min_mag: f32, max_mag: f32) -> Self {
        Self {
            magic: STAR_CATALOG_MAGIC,
            version: STAR_CATALOG_VERSION,
            star_count,
            min_mag,
            max_mag,
            padding: [0u8; 12],
        }
    }

    pub fn validate(&self) -> Result<(), std::io::Error> {
        if self.magic != STAR_CATALOG_MAGIC {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Invalid magic bytes: {:?}", self.magic),
            ));
        }
        if self.version > STAR_CATALOG_VERSION {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "Unsupported version: {}, expected {} or older",
                    self.version, STAR_CATALOG_VERSION
                ),
            ));
        }
        Ok(())
    }
}

pub fn read_stars_from_zst(
    reader: impl Read,
) -> Result<(StarCatalogHeader, Vec<StarRecord>), std::io::Error> {
    let mut decoder = zstd::stream::Decoder::new(reader)?;

    let mut header = StarCatalogHeader::zeroed();
    decoder.read_exact(bytemuck::bytes_of_mut(&mut header))?;
    header.validate()?;

    let count = header.star_count as usize;
    let expected_bytes = count * std::mem::size_of::<StarRecord>();
    let mut raw_bytes = vec![0u8; expected_bytes];
    decoder.read_exact(&mut raw_bytes)?;

    let records: Vec<StarRecord> = bytemuck::cast_slice(&raw_bytes).to_vec();
    Ok((header, records))
}

pub fn write_stars_to_zst(
    writer: impl Write,
    header: &StarCatalogHeader,
    stars: &[StarRecord],
    compression_level: i32,
) -> Result<(), std::io::Error> {
    let mut encoder = zstd::stream::Encoder::new(writer, compression_level)?;
    encoder.write_all(bytemuck::bytes_of(header))?;
    encoder.write_all(bytemuck::cast_slice(stars))?;
    encoder.finish()?;
    Ok(())
}

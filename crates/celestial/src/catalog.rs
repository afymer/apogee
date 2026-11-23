use bytemuck::Zeroable;
use std::io::{self, Read, Write};

pub const STAR_CATALOG_MAGIC: [u8; 4] = *b"APST";
pub const STAR_CATALOG_VERSION: u32 = 2;

const META_FLAG_PROPER_NAME: u8 = 0x01;
const META_FLAG_BAYER_FLAMSTEED: u8 = 0x02;
const META_FLAG_HIP: u8 = 0x04;
const META_FLAG_HD: u8 = 0x08;
const META_FLAG_HR: u8 = 0x10;
const META_FLAG_TYC: u8 = 0x20;
const META_FLAG_ALL: u8 = META_FLAG_PROPER_NAME
    | META_FLAG_BAYER_FLAMSTEED
    | META_FLAG_HIP
    | META_FLAG_HD
    | META_FLAG_HR
    | META_FLAG_TYC;

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
        let pos = crate::coords::canonical_spherical_to_cartesian(
            crate::coords::Angle::from_radians(ra_rad),
            crate::coords::Angle::from_radians(dec_rad),
        )
        .as_vec3()
        .to_array();

        Self::new(pos, mag, color_index)
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

    pub fn validate(&self) -> Result<(), io::Error> {
        if self.magic != STAR_CATALOG_MAGIC {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Invalid magic bytes: {:?}", self.magic),
            ));
        }
        if self.version == 0 || self.version > STAR_CATALOG_VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "Unsupported version: {}, expected 1..={STAR_CATALOG_VERSION}",
                    self.version
                ),
            ));
        }
        Ok(())
    }
}

/// Optional human-readable names and catalog cross-identifications for a star.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StarMetadata {
    /// Official IAU proper name (e.g. "Sirius", "Alioth", "Polaris").
    pub proper_name: Option<String>,
    /// Bayer or Flamsteed designation in Unicode (e.g. "α CMa", "ε UMa", "61 Cyg").
    pub bayer_flamsteed: Option<String>,
    /// Hipparcos catalog identifier (e.g. 32349 -> "HIP 32349").
    pub hip: Option<u32>,
    /// Henry Draper catalog identifier (e.g. 48915 -> "HD 48915").
    pub hd: Option<u32>,
    /// Harvard Revised / Yale Bright Star Catalog number (e.g. 2491 -> "HR 2491").
    pub hr: Option<u16>,
    /// Tycho-2 catalog identifier without "TYC " prefix (e.g. "3663-2668-1").
    pub tyc: Option<String>,
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.filter(|s| !s.is_empty())
}

impl StarMetadata {
    /// Returns `true` when no name or catalog designation is set.
    pub fn is_empty(&self) -> bool {
        non_empty(self.proper_name.as_deref()).is_none()
            && non_empty(self.bayer_flamsteed.as_deref()).is_none()
            && self.hip.is_none()
            && self.hd.is_none()
            && self.hr.is_none()
            && non_empty(self.tyc.as_deref()).is_none()
    }

    /// Returns the highest-priority primary display label, if any designation exists.
    pub fn primary_name(&self) -> Option<String> {
        if let Some(name) = non_empty(self.proper_name.as_deref()) {
            return Some(name.to_string());
        }
        if let Some(bayer) = non_empty(self.bayer_flamsteed.as_deref()) {
            return Some(bayer.to_string());
        }
        if let Some(hip) = self.hip {
            return Some(format!("HIP {hip}"));
        }
        if let Some(hd) = self.hd {
            return Some(format!("HD {hd}"));
        }
        if let Some(hr) = self.hr {
            return Some(format!("HR {hr}"));
        }
        if let Some(tyc) = non_empty(self.tyc.as_deref()) {
            return Some(format!("TYC {tyc}"));
        }
        None
    }

    /// Returns up to 2 distinct secondary designations excluding whichever field supplied [`Self::primary_name`].
    pub fn secondary_designations(&self) -> Vec<String> {
        let mut out = Vec::with_capacity(2);
        if non_empty(self.proper_name.as_deref()).is_some() {
            if let Some(bayer) = non_empty(self.bayer_flamsteed.as_deref()) {
                out.push(bayer.to_string());
            }
            if let Some(hip) = self.hip {
                out.push(format!("HIP {hip}"));
            } else if let Some(hd) = self.hd {
                out.push(format!("HD {hd}"));
            } else if let Some(hr) = self.hr {
                out.push(format!("HR {hr}"));
            }
        } else if non_empty(self.bayer_flamsteed.as_deref()).is_some() {
            if let Some(hip) = self.hip {
                out.push(format!("HIP {hip}"));
            }
            if let Some(hd) = self.hd {
                out.push(format!("HD {hd}"));
            } else if let Some(hr) = self.hr {
                out.push(format!("HR {hr}"));
            }
        } else if self.hip.is_some() {
            if let Some(hd) = self.hd {
                out.push(format!("HD {hd}"));
            } else if let Some(hr) = self.hr {
                out.push(format!("HR {hr}"));
            }
        } else if self.hd.is_some() {
            if let Some(hr) = self.hr {
                out.push(format!("HR {hr}"));
            }
        }
        out.truncate(2);
        out
    }

    /// Formats a primary hover title and optional metadata subtitle incorporating `spectral_class`.
    pub fn display_title_and_subtitle(&self, spectral_class: &str) -> (String, Option<String>) {
        if let Some(title) = self.primary_name() {
            let mut parts = self.secondary_designations();
            if parts.is_empty() {
                parts.push(format!("{spectral_class}-type Star"));
            } else {
                parts.push(format!("{spectral_class}-type"));
            }
            (title, Some(parts.join(" · ")))
        } else {
            (format!("{spectral_class}-type Star"), None)
        }
    }
}

/// Decoded Apogee star catalog containing compact GPU star records and sparse CPU metadata.
#[derive(Clone, Debug, PartialEq)]
pub struct StarCatalog {
    pub header: StarCatalogHeader,
    pub records: Vec<StarRecord>,
    /// Sparse metadata entries sorted strictly ascending by `star_index`.
    pub metadata: Vec<(u32, StarMetadata)>,
}

impl StarCatalog {
    /// Looks up optional naming metadata for `star_index` in `O(log N)` time.
    pub fn metadata_for(&self, star_index: u32) -> Option<&StarMetadata> {
        self.metadata
            .binary_search_by_key(&star_index, |(idx, _)| *idx)
            .ok()
            .map(|pos| &self.metadata[pos].1)
    }
}

fn read_u16_le(reader: &mut impl Read) -> Result<u16, io::Error> {
    let mut buf = [0u8; 2];
    reader.read_exact(&mut buf)?;
    Ok(u16::from_le_bytes(buf))
}

fn read_u32_le(reader: &mut impl Read) -> Result<u32, io::Error> {
    let mut buf = [0u8; 4];
    reader.read_exact(&mut buf)?;
    Ok(u32::from_le_bytes(buf))
}

fn read_short_string(reader: &mut impl Read) -> Result<String, io::Error> {
    let mut len_buf = [0u8; 1];
    reader.read_exact(&mut len_buf)?;
    let len = len_buf[0] as usize;
    let mut bytes = vec![0u8; len];
    reader.read_exact(&mut bytes)?;
    String::from_utf8(bytes).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Invalid UTF-8 in star metadata string: {e}"),
        )
    })
}

fn write_short_string(writer: &mut impl Write, s: &str) -> Result<(), io::Error> {
    let len = u8::try_from(s.len()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("Star metadata string exceeds 255 bytes: {} bytes", s.len()),
        )
    })?;
    writer.write_all(&[len])?;
    writer.write_all(s.as_bytes())
}

fn decode_header_and_records(
    decoder: &mut zstd::stream::Decoder<'_, impl std::io::BufRead>,
) -> Result<(StarCatalogHeader, Vec<StarRecord>), io::Error> {
    const MAX_STAR_COUNT: u32 = 50_000_000;

    let mut header = StarCatalogHeader::zeroed();
    decoder.read_exact(bytemuck::bytes_of_mut(&mut header))?;
    header.validate()?;

    if header.star_count > MAX_STAR_COUNT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "Star count {} exceeds maximum supported ({MAX_STAR_COUNT})",
                header.star_count
            ),
        ));
    }

    let count = header.star_count as usize;
    let _expected_bytes = count
        .checked_mul(std::mem::size_of::<StarRecord>())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "Star catalog size overflows usize",
            )
        })?;
    let mut records = vec![StarRecord::zeroed(); count];
    decoder.read_exact(bytemuck::cast_slice_mut(&mut records))?;
    Ok((header, records))
}

/// Reads header and star records from a v1 or v2 zstd-compressed Apogee star catalog.
pub fn read_stars_from_zst(
    reader: impl Read,
) -> Result<(StarCatalogHeader, Vec<StarRecord>), io::Error> {
    let mut decoder = zstd::stream::Decoder::new(reader)?;
    decode_header_and_records(&mut decoder)
}

/// Reads a full star catalog (v1 or v2), including sparse v2 naming and designation metadata.
pub fn read_star_catalog_from_zst(reader: impl Read) -> Result<StarCatalog, io::Error> {
    let mut decoder = zstd::stream::Decoder::new(reader)?;
    let (header, records) = decode_header_and_records(&mut decoder)?;

    if header.version == 1 {
        return Ok(StarCatalog {
            header,
            records,
            metadata: Vec::new(),
        });
    }

    let meta_count = read_u32_le(&mut decoder)?;
    if meta_count > header.star_count {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "Metadata count ({meta_count}) exceeds star count ({})",
                header.star_count
            ),
        ));
    }

    let mut metadata = Vec::with_capacity(meta_count as usize);
    let mut prev_index: Option<u32> = None;

    for _ in 0..meta_count {
        let star_index = read_u32_le(&mut decoder)?;
        if star_index >= header.star_count {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "Metadata star index {star_index} out of bounds for {} stars",
                    header.star_count
                ),
            ));
        }
        if prev_index.is_some_and(|prev| star_index <= prev) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Metadata indices are not strictly ascending near index {star_index}"),
            ));
        }
        prev_index = Some(star_index);

        let mut flag_buf = [0u8; 1];
        decoder.read_exact(&mut flag_buf)?;
        let flags = flag_buf[0];
        if flags & !META_FLAG_ALL != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Unsupported star metadata flags: {flags:#04x}"),
            ));
        }

        let mut meta = StarMetadata::default();
        if flags & META_FLAG_PROPER_NAME != 0 {
            meta.proper_name = Some(read_short_string(&mut decoder)?);
        }
        if flags & META_FLAG_BAYER_FLAMSTEED != 0 {
            meta.bayer_flamsteed = Some(read_short_string(&mut decoder)?);
        }
        if flags & META_FLAG_HIP != 0 {
            meta.hip = Some(read_u32_le(&mut decoder)?);
        }
        if flags & META_FLAG_HD != 0 {
            meta.hd = Some(read_u32_le(&mut decoder)?);
        }
        if flags & META_FLAG_HR != 0 {
            meta.hr = Some(read_u16_le(&mut decoder)?);
        }
        if flags & META_FLAG_TYC != 0 {
            meta.tyc = Some(read_short_string(&mut decoder)?);
        }

        metadata.push((star_index, meta));
    }

    Ok(StarCatalog {
        header,
        records,
        metadata,
    })
}

fn write_catalog_inner(
    writer: impl Write,
    header: &StarCatalogHeader,
    stars: &[StarRecord],
    metadata: &[(u32, StarMetadata)],
    compression_level: i32,
) -> Result<(), io::Error> {
    if header.star_count as usize != stars.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "Star catalog header count ({}) does not match slice length ({})",
                header.star_count,
                stars.len()
            ),
        ));
    }

    let mut filtered: Vec<(u32, &StarMetadata)> = Vec::new();
    if header.version >= 2 {
        let mut prev_index: Option<u32> = None;
        for (idx, meta) in metadata {
            if *idx >= header.star_count {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "Metadata star index {idx} out of bounds for {} stars",
                        header.star_count
                    ),
                ));
            }
            if prev_index.is_some_and(|prev| *idx <= prev) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("Metadata indices must be strictly ascending (saw {idx})"),
                ));
            }
            prev_index = Some(*idx);

            if !meta.is_empty() {
                for s in [
                    non_empty(meta.proper_name.as_deref()),
                    non_empty(meta.bayer_flamsteed.as_deref()),
                    non_empty(meta.tyc.as_deref()),
                ]
                .into_iter()
                .flatten()
                {
                    if s.len() > u8::MAX as usize {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidInput,
                            format!("Star metadata string exceeds 255 bytes: {} bytes", s.len()),
                        ));
                    }
                }
                filtered.push((*idx, meta));
            }
        }
    }

    let mut encoder = zstd::stream::Encoder::new(writer, compression_level)?;
    encoder.write_all(bytemuck::bytes_of(header))?;
    encoder.write_all(bytemuck::cast_slice(stars))?;

    if header.version >= 2 {
        encoder.write_all(&(filtered.len() as u32).to_le_bytes())?;
        for (star_index, meta) in filtered {
            encoder.write_all(&star_index.to_le_bytes())?;

            let mut flags = 0u8;
            if non_empty(meta.proper_name.as_deref()).is_some() {
                flags |= META_FLAG_PROPER_NAME;
            }
            if non_empty(meta.bayer_flamsteed.as_deref()).is_some() {
                flags |= META_FLAG_BAYER_FLAMSTEED;
            }
            if meta.hip.is_some() {
                flags |= META_FLAG_HIP;
            }
            if meta.hd.is_some() {
                flags |= META_FLAG_HD;
            }
            if meta.hr.is_some() {
                flags |= META_FLAG_HR;
            }
            if non_empty(meta.tyc.as_deref()).is_some() {
                flags |= META_FLAG_TYC;
            }
            encoder.write_all(&[flags])?;

            if let Some(name) = non_empty(meta.proper_name.as_deref()) {
                write_short_string(&mut encoder, name)?;
            }
            if let Some(bayer) = non_empty(meta.bayer_flamsteed.as_deref()) {
                write_short_string(&mut encoder, bayer)?;
            }
            if let Some(hip) = meta.hip {
                encoder.write_all(&hip.to_le_bytes())?;
            }
            if let Some(hd) = meta.hd {
                encoder.write_all(&hd.to_le_bytes())?;
            }
            if let Some(hr) = meta.hr {
                encoder.write_all(&hr.to_le_bytes())?;
            }
            if let Some(tyc) = non_empty(meta.tyc.as_deref()) {
                write_short_string(&mut encoder, tyc)?;
            }
        }
    }

    encoder.finish()?;
    Ok(())
}

/// Writes star records to a zstd-compressed catalog without metadata.
pub fn write_stars_to_zst(
    writer: impl Write,
    header: &StarCatalogHeader,
    stars: &[StarRecord],
    compression_level: i32,
) -> Result<(), io::Error> {
    write_catalog_inner(writer, header, stars, &[], compression_level)
}

/// Writes a full v2 (or v1) star catalog with sparse naming and designation metadata.
pub fn write_star_catalog_to_zst(
    writer: impl Write,
    header: &StarCatalogHeader,
    stars: &[StarRecord],
    metadata: &[(u32, StarMetadata)],
    compression_level: i32,
) -> Result<(), io::Error> {
    header.validate()?;
    write_catalog_inner(writer, header, stars, metadata, compression_level)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_v1_catalog_with_empty_metadata() {
        let stars = vec![
            StarRecord::from_icrs_spherical(1.25, 0.45, -1.46, 0.01),
            StarRecord::from_icrs_spherical(5.10, -0.80, 6.24, 1.35),
        ];
        let mut header = StarCatalogHeader::new(stars.len() as u32, -1.46, 6.24);
        header.version = 1;
        assert!(header.validate().is_ok());

        let mut buf = Vec::new();
        write_stars_to_zst(&mut buf, &header, &stars, 1).expect("write v1");

        let catalog = read_star_catalog_from_zst(buf.as_slice()).expect("read v1 as StarCatalog");
        assert_eq!(catalog.header.version, 1);
        assert_eq!(catalog.records, stars);
        assert_eq!(catalog.metadata, Vec::new());
        assert!(catalog.metadata_for(0).is_none());
    }

    #[test]
    fn v2_catalog_roundtrip_and_records_only_reader() {
        let stars = vec![
            StarRecord::from_icrs_spherical(1.76, -0.29, -1.46, 0.01),
            StarRecord::from_icrs_spherical(0.24, 1.06, 2.06, 0.41),
            StarRecord::from_icrs_spherical(3.10, 0.12, 7.80, 1.15),
        ];
        let header = StarCatalogHeader::new(stars.len() as u32, -1.46, 7.80);
        let metadata = vec![
            (
                0,
                StarMetadata {
                    proper_name: Some("Sirius".to_string()),
                    bayer_flamsteed: Some("α CMa".to_string()),
                    hip: Some(32349),
                    hd: Some(48915),
                    hr: Some(2491),
                    tyc: None,
                },
            ),
            (
                1,
                StarMetadata {
                    proper_name: None,
                    bayer_flamsteed: Some("γ Cas".to_string()),
                    hip: Some(4427),
                    hd: Some(5394),
                    hr: Some(264),
                    tyc: None,
                },
            ),
            (2, StarMetadata::default()),
        ];

        let mut buf = Vec::new();
        write_star_catalog_to_zst(&mut buf, &header, &stars, &metadata, 3).expect("write v2");

        let catalog = read_star_catalog_from_zst(buf.as_slice()).expect("read v2");
        assert_eq!(catalog.header, header);
        assert_eq!(catalog.records, stars);
        assert_eq!(catalog.metadata.len(), 2);
        assert_eq!(catalog.metadata_for(0), Some(&metadata[0].1));
        assert_eq!(catalog.metadata_for(1), Some(&metadata[1].1));
        assert_eq!(catalog.metadata_for(2), None);

        let (hdr_only, recs_only) = read_stars_from_zst(buf.as_slice()).expect("read records only");
        assert_eq!(hdr_only, header);
        assert_eq!(recs_only, stars);
    }

    #[test]
    fn star_metadata_display_formatting_tiers() {
        let sirius = StarMetadata {
            proper_name: Some("Sirius".to_string()),
            bayer_flamsteed: Some("α CMa".to_string()),
            hip: Some(32349),
            hd: Some(48915),
            hr: Some(2491),
            tyc: None,
        };
        assert_eq!(
            sirius.display_title_and_subtitle("A"),
            (
                "Sirius".to_string(),
                Some("α CMa · HIP 32349 · A-type".to_string())
            )
        );

        let gamma_cas = StarMetadata {
            bayer_flamsteed: Some("γ Cas".to_string()),
            hip: Some(4427),
            hd: Some(5394),
            ..Default::default()
        };
        assert_eq!(
            gamma_cas.display_title_and_subtitle("B"),
            (
                "γ Cas".to_string(),
                Some("HIP 4427 · HD 5394 · B-type".to_string())
            )
        );

        let hip_hd = StarMetadata {
            hip: Some(12345),
            hd: Some(67890),
            ..Default::default()
        };
        assert_eq!(
            hip_hd.display_title_and_subtitle("K"),
            (
                "HIP 12345".to_string(),
                Some("HD 67890 · K-type".to_string())
            )
        );

        let hip_only = StarMetadata {
            hip: Some(54321),
            ..Default::default()
        };
        assert_eq!(
            hip_only.display_title_and_subtitle("F"),
            ("HIP 54321".to_string(), Some("F-type Star".to_string()))
        );

        let tyc_only = StarMetadata {
            tyc: Some("3663-2668-1".to_string()),
            ..Default::default()
        };
        assert_eq!(
            tyc_only.display_title_and_subtitle("G"),
            (
                "TYC 3663-2668-1".to_string(),
                Some("G-type Star".to_string())
            )
        );

        let unnamed = StarMetadata::default();
        assert_eq!(
            unnamed.display_title_and_subtitle("M"),
            ("M-type Star".to_string(), None)
        );
    }
}

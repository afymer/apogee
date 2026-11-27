use std::io::{self, Read, Write};

use bytemuck::Zeroable;
use glam::DVec3;

use crate::coords::{Angle, canonical_spherical_to_cartesian};

pub const DSO_CATALOG_MAGIC: [u8; 4] = *b"APDS";
pub const DSO_CATALOG_VERSION: u32 = 1;
const MAX_DSO_COUNT: u32 = 1_000_000;

const FLAG_MESSIER: u16 = 1 << 0;
const FLAG_NGC: u16 = 1 << 1;
const FLAG_IC: u16 = 1 << 2;
const FLAG_ALT_NGC: u16 = 1 << 3;
const FLAG_COMMON_NAMES: u16 = 1 << 4;
const FLAG_V_MAG: u16 = 1 << 5;
const FLAG_B_MAG: u16 = 1 << 6;
const FLAG_SURFACE_BRIGHTNESS: u16 = 1 << 7;
const FLAG_MAJOR_ARCMIN: u16 = 1 << 8;
const FLAG_MINOR_ARCMIN: u16 = 1 << 9;
const FLAG_POSITION_ANGLE: u16 = 1 << 10;
const FLAG_HUBBLE_TYPE: u16 = 1 << 11;
const FLAG_ALL: u16 = FLAG_MESSIER
    | FLAG_NGC
    | FLAG_IC
    | FLAG_ALT_NGC
    | FLAG_COMMON_NAMES
    | FLAG_V_MAG
    | FLAG_B_MAG
    | FLAG_SURFACE_BRIGHTNESS
    | FLAG_MAJOR_ARCMIN
    | FLAG_MINOR_ARCMIN
    | FLAG_POSITION_ANGLE
    | FLAG_HUBBLE_TYPE;

/// 32-byte header at the start of an Apogee Deep-Sky Object Catalog file.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct DsoCatalogHeader {
    pub magic: [u8; 4],
    pub version: u32,
    pub object_count: u32,
    pub messier_count: u32,
    pub ngc_count: u32,
    pub padding: [u8; 12],
}

impl DsoCatalogHeader {
    pub fn new(object_count: u32, messier_count: u32, ngc_count: u32) -> Self {
        Self {
            magic: DSO_CATALOG_MAGIC,
            version: DSO_CATALOG_VERSION,
            object_count,
            messier_count,
            ngc_count,
            padding: [0u8; 12],
        }
    }

    pub fn validate(&self) -> Result<(), io::Error> {
        if self.magic != DSO_CATALOG_MAGIC {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Invalid DSO magic bytes: {:?}", self.magic),
            ));
        }
        if self.version == 0 || self.version > DSO_CATALOG_VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "Unsupported DSO catalog version: {}, expected 1..={DSO_CATALOG_VERSION}",
                    self.version
                ),
            ));
        }
        if self.object_count > MAX_DSO_COUNT {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "DSO count {} exceeds maximum supported ({MAX_DSO_COUNT})",
                    self.object_count
                ),
            ));
        }
        Ok(())
    }
}

/// Broad morphological/astronomical grouping of deep-sky objects for search filtering.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DsoGroup {
    Galaxy,
    Nebula,
    PlanetaryNebula,
    OpenCluster,
    GlobularCluster,
    Other,
}

impl DsoGroup {
    pub const ALL: [Self; 6] = [
        Self::Galaxy,
        Self::Nebula,
        Self::PlanetaryNebula,
        Self::OpenCluster,
        Self::GlobularCluster,
        Self::Other,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Galaxy => "Galaxy",
            Self::Nebula => "Nebula",
            Self::PlanetaryNebula => "Planetary",
            Self::OpenCluster => "Open",
            Self::GlobularCluster => "Globular",
            Self::Other => "Other",
        }
    }

    pub fn srgb(self) -> [f32; 3] {
        match self {
            Self::Galaxy => [0.92, 0.44, 0.42],
            Self::Nebula => [0.35, 0.85, 0.56],
            Self::PlanetaryNebula => [0.30, 0.86, 0.88],
            Self::OpenCluster => [0.95, 0.80, 0.36],
            Self::GlobularCluster => [0.96, 0.68, 0.28],
            Self::Other => [0.72, 0.66, 0.92],
        }
    }
}

/// Specific OpenNGC-aligned deep-sky object classification.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DsoKind {
    Galaxy = 0,
    GalaxyPair = 1,
    GalaxyTriplet = 2,
    GalaxyGroup = 3,
    OpenCluster = 4,
    GlobularCluster = 5,
    ClusterNebula = 6,
    PlanetaryNebula = 7,
    Nebula = 8,
    HiiRegion = 9,
    EmissionNebula = 10,
    ReflectionNebula = 11,
    DarkNebula = 12,
    SupernovaRemnant = 13,
    StarAssociation = 14,
    DoubleStar = 15,
    Star = 16,
    Other = 17,
}

impl DsoKind {
    pub fn from_u8(val: u8) -> Option<Self> {
        match val {
            0 => Some(Self::Galaxy),
            1 => Some(Self::GalaxyPair),
            2 => Some(Self::GalaxyTriplet),
            3 => Some(Self::GalaxyGroup),
            4 => Some(Self::OpenCluster),
            5 => Some(Self::GlobularCluster),
            6 => Some(Self::ClusterNebula),
            7 => Some(Self::PlanetaryNebula),
            8 => Some(Self::Nebula),
            9 => Some(Self::HiiRegion),
            10 => Some(Self::EmissionNebula),
            11 => Some(Self::ReflectionNebula),
            12 => Some(Self::DarkNebula),
            13 => Some(Self::SupernovaRemnant),
            14 => Some(Self::StarAssociation),
            15 => Some(Self::DoubleStar),
            16 => Some(Self::Star),
            17 => Some(Self::Other),
            _ => None,
        }
    }

    pub fn from_openngc_code(code: &str) -> Option<Self> {
        match code.trim() {
            "G" => Some(Self::Galaxy),
            "GPair" => Some(Self::GalaxyPair),
            "GTrpl" => Some(Self::GalaxyTriplet),
            "GGroup" => Some(Self::GalaxyGroup),
            "OCl" => Some(Self::OpenCluster),
            "GCl" => Some(Self::GlobularCluster),
            "Cl+N" => Some(Self::ClusterNebula),
            "PN" => Some(Self::PlanetaryNebula),
            "Neb" => Some(Self::Nebula),
            "HII" => Some(Self::HiiRegion),
            "EmN" => Some(Self::EmissionNebula),
            "RfN" => Some(Self::ReflectionNebula),
            "DrkN" => Some(Self::DarkNebula),
            "SNR" => Some(Self::SupernovaRemnant),
            "*Ass" => Some(Self::StarAssociation),
            "**" => Some(Self::DoubleStar),
            "*" => Some(Self::Star),
            "Other" => Some(Self::Other),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Galaxy => "Galaxy",
            Self::GalaxyPair => "Galaxy Pair",
            Self::GalaxyTriplet => "Galaxy Triplet",
            Self::GalaxyGroup => "Galaxy Group",
            Self::OpenCluster => "Open Cluster",
            Self::GlobularCluster => "Globular Cluster",
            Self::ClusterNebula => "Cluster + Nebula",
            Self::PlanetaryNebula => "Planetary Nebula",
            Self::Nebula => "Nebula",
            Self::HiiRegion => "H II Region",
            Self::EmissionNebula => "Emission Nebula",
            Self::ReflectionNebula => "Reflection Nebula",
            Self::DarkNebula => "Dark Nebula",
            Self::SupernovaRemnant => "Supernova Remnant",
            Self::StarAssociation => "Star Association",
            Self::DoubleStar => "Double Star",
            Self::Star => "Star",
            Self::Other => "Other",
        }
    }

    pub fn short_label(self) -> &'static str {
        match self {
            Self::Galaxy | Self::GalaxyPair | Self::GalaxyTriplet | Self::GalaxyGroup => "Galaxy",
            Self::OpenCluster => "Open Cluster",
            Self::GlobularCluster => "Globular",
            Self::ClusterNebula => "Cluster + Neb",
            Self::PlanetaryNebula => "Planetary",
            Self::Nebula
            | Self::HiiRegion
            | Self::EmissionNebula
            | Self::ReflectionNebula
            | Self::DarkNebula => "Nebula",
            Self::SupernovaRemnant => "SNR",
            Self::StarAssociation => "Association",
            Self::DoubleStar => "Double Star",
            Self::Star => "Star",
            Self::Other => "DSO",
        }
    }

    pub fn group(self) -> DsoGroup {
        match self {
            Self::Galaxy | Self::GalaxyPair | Self::GalaxyTriplet | Self::GalaxyGroup => {
                DsoGroup::Galaxy
            }
            Self::OpenCluster | Self::StarAssociation => DsoGroup::OpenCluster,
            Self::GlobularCluster => DsoGroup::GlobularCluster,
            Self::PlanetaryNebula => DsoGroup::PlanetaryNebula,
            Self::ClusterNebula
            | Self::Nebula
            | Self::HiiRegion
            | Self::EmissionNebula
            | Self::ReflectionNebula
            | Self::DarkNebula
            | Self::SupernovaRemnant => DsoGroup::Nebula,
            Self::DoubleStar | Self::Star | Self::Other => DsoGroup::Other,
        }
    }

    /// Astronomical chart symbol variant rendered by `celestial_dso.wgsl`.
    /// - `0`: Galaxy ellipse
    /// - `1`: Open cluster / Star association dashed circle
    /// - `2`: Globular cluster circle with inner crosshairs
    /// - `3`: Planetary nebula circle with 4 external ticks
    /// - `4`: Nebula rounded square
    /// - `5`: Cluster + Nebula square + inner dashed circle
    /// - `6`: Supernova remnant dashed diamond/ring
    /// - `7`: Double star / Star / Other small diamond
    pub fn symbol_code(self) -> u8 {
        match self {
            Self::Galaxy | Self::GalaxyPair | Self::GalaxyTriplet | Self::GalaxyGroup => 0,
            Self::OpenCluster | Self::StarAssociation => 1,
            Self::GlobularCluster => 2,
            Self::PlanetaryNebula => 3,
            Self::Nebula
            | Self::HiiRegion
            | Self::EmissionNebula
            | Self::ReflectionNebula
            | Self::DarkNebula => 4,
            Self::ClusterNebula => 5,
            Self::SupernovaRemnant => 6,
            Self::DoubleStar | Self::Star | Self::Other => 7,
        }
    }

    pub fn color_id(self) -> u8 {
        match self {
            Self::Galaxy | Self::GalaxyPair | Self::GalaxyTriplet | Self::GalaxyGroup => 0,
            Self::OpenCluster | Self::StarAssociation => 1,
            Self::GlobularCluster => 2,
            Self::PlanetaryNebula => 3,
            Self::Nebula | Self::HiiRegion | Self::EmissionNebula | Self::SupernovaRemnant => 4,
            Self::ReflectionNebula => 5,
            Self::DarkNebula => 6,
            Self::ClusterNebula => 7,
            Self::DoubleStar | Self::Star | Self::Other => 8,
        }
    }

    pub fn srgb(self) -> [f32; 3] {
        match self.color_id() {
            0 => [0.92, 0.44, 0.42],
            1 => [0.95, 0.80, 0.36],
            2 => [0.96, 0.68, 0.28],
            3 => [0.30, 0.86, 0.88],
            4 => [0.35, 0.85, 0.56],
            5 => [0.45, 0.72, 0.96],
            6 => [0.62, 0.66, 0.74],
            7 => [0.46, 0.88, 0.66],
            _ => [0.72, 0.66, 0.92],
        }
    }
}

/// Complete Messier / NGC deep-sky catalog record in ICRS J2000 coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct DsoRecord {
    pub ra_deg: f64,
    pub dec_deg: f64,
    pub kind: DsoKind,
    pub constellation: [u8; 3],
    pub messier: Option<u16>,
    pub ngc: Option<String>,
    pub ic: Option<String>,
    pub alt_ngc: Vec<String>,
    pub common_names: Vec<String>,
    pub v_mag: Option<f32>,
    pub b_mag: Option<f32>,
    pub surface_brightness: Option<f32>,
    pub major_arcmin: Option<f32>,
    pub minor_arcmin: Option<f32>,
    pub position_angle_deg: Option<f32>,
    pub hubble_type: Option<String>,
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|s| !s.is_empty())
}

impl DsoRecord {
    pub fn icrs_dir(&self) -> DVec3 {
        canonical_spherical_to_cartesian(
            Angle::from_degrees(self.ra_deg),
            Angle::from_degrees(self.dec_deg),
        )
        .normalize_or_zero()
    }

    pub fn constellation_str(&self) -> Option<&str> {
        if self.constellation == [0, 0, 0] {
            return None;
        }
        std::str::from_utf8(&self.constellation)
            .ok()
            .map(|s| s.trim_end_matches('\0'))
            .filter(|s| !s.is_empty())
    }

    #[inline]
    pub fn is_messier(&self) -> bool {
        self.messier.is_some()
    }

    #[inline]
    pub fn is_ngc(&self) -> bool {
        non_empty(self.ngc.as_deref()).is_some() || !self.alt_ngc.is_empty()
    }

    pub fn messier_designation(&self) -> Option<String> {
        self.messier.map(|m| format!("M{m}"))
    }

    pub fn ngc_designation(&self) -> Option<String> {
        non_empty(self.ngc.as_deref()).map(|id| format!("NGC {id}"))
    }

    pub fn ic_designation(&self) -> Option<String> {
        non_empty(self.ic.as_deref()).map(|id| format!("IC {id}"))
    }

    pub fn primary_designation(&self) -> String {
        if let Some(m) = self.messier_designation() {
            return m;
        }
        if let Some(ngc) = self.ngc_designation() {
            return ngc;
        }
        if let Some(ic) = self.ic_designation() {
            return ic;
        }
        if let Some(first_alt) = self
            .alt_ngc
            .first()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            return format!("NGC {first_alt}");
        }
        if let Some(name) = self.primary_common_name() {
            return name.to_string();
        }
        "DSO".to_string()
    }

    pub fn primary_common_name(&self) -> Option<&str> {
        self.common_names
            .iter()
            .map(String::as_str)
            .map(str::trim)
            .find(|s| !s.is_empty())
    }

    pub fn hover_title(&self) -> String {
        let primary = self.primary_designation();
        if let Some(name) = self.primary_common_name()
            && !name.eq_ignore_ascii_case(&primary)
        {
            format!("{primary} · {name}")
        } else {
            primary
        }
    }

    pub fn display_title_and_subtitle(&self) -> (String, Option<String>) {
        let title = self.primary_designation();
        let mut parts: Vec<String> = Vec::with_capacity(4);

        if let Some(common) = self.primary_common_name()
            && !common.eq_ignore_ascii_case(&title)
        {
            parts.push(common.to_string());
        }

        let secondary_id = if self.is_messier() {
            self.ngc_designation().or_else(|| self.ic_designation())
        } else if self.ngc.is_some() {
            self.ic_designation()
        } else {
            None
        };
        if let Some(id) = secondary_id {
            parts.push(id);
        }

        if let Some(hubble) = non_empty(self.hubble_type.as_deref()) {
            parts.push(hubble.to_string());
        }
        if let Some(con) = self.constellation_str() {
            parts.push(con.to_string());
        }

        let subtitle = (!parts.is_empty()).then(|| parts.join(" · "));
        (title, subtitle)
    }

    pub fn catalog_ids_summary(&self) -> Option<String> {
        let mut ids: Vec<String> = Vec::new();
        if let Some(m) = self.messier_designation() {
            ids.push(m);
        }
        if let Some(ngc) = self.ngc_designation() {
            ids.push(ngc);
        }
        for alt in &self.alt_ngc {
            let trimmed = alt.trim();
            if !trimmed.is_empty() {
                let formatted = format!("NGC {trimmed}");
                if !ids.contains(&formatted) {
                    ids.push(formatted);
                }
            }
        }
        if let Some(ic) = self.ic_designation() {
            ids.push(ic);
        }
        (!ids.is_empty()).then(|| ids.join(" · "))
    }

    pub fn mag_band_and_value(&self) -> Option<(&'static str, f32)> {
        if let Some(v) = self.v_mag.filter(|m| m.is_finite()) {
            Some(("V", v))
        } else if let Some(b) = self.b_mag.filter(|m| m.is_finite()) {
            Some(("B", b))
        } else {
            None
        }
    }

    pub fn sort_mag(&self) -> Option<f32> {
        self.v_mag
            .filter(|m| m.is_finite())
            .or_else(|| self.b_mag.filter(|m| m.is_finite()))
    }

    pub fn mag_label(&self) -> String {
        match self.mag_band_and_value() {
            Some((band, val)) => format!("{band} {val:.1}"),
            None => "—".to_string(),
        }
    }

    pub fn mag_detail_label(&self) -> String {
        match self.mag_band_and_value() {
            Some((band, val)) => format!("{band} {val:.2}"),
            None => "—".to_string(),
        }
    }

    pub fn angular_size_label(&self) -> Option<String> {
        let maj = self.major_arcmin.filter(|v| v.is_finite() && *v > 0.0)?;
        let format_dim = |arcmin: f32| -> String {
            if arcmin >= 1.0 {
                format!("{arcmin:.1}'")
            } else {
                format!("{:.0}\"", (arcmin * 60.0).max(1.0))
            }
        };

        if let Some(min) = self.minor_arcmin.filter(|v| v.is_finite() && *v > 0.0)
            && (maj - min).abs() > 0.05 * maj.max(1.0)
        {
            Some(format!("{} × {}", format_dim(maj), format_dim(min)))
        } else {
            Some(format_dim(maj))
        }
    }

    /// Apparent magnitude used for progressive LOD display and shader culling.
    /// Falls back to `B - 0.5`, then an angular-size heuristic for emission/reflection nebulae
    /// lacking integrated photometry, and caps Messier objects at 9.5 so every Messier remains
    /// accessible at moderate zoom levels.
    pub fn effective_display_mag(&self) -> f32 {
        let raw = if let Some(v) = self.v_mag.filter(|m| m.is_finite()) {
            v
        } else if let Some(b) = self.b_mag.filter(|m| m.is_finite()) {
            b - 0.5
        } else {
            let maj = self.major_arcmin.filter(|m| m.is_finite()).unwrap_or(0.0);
            if maj >= 60.0 {
                6.5
            } else if maj >= 25.0 {
                8.0
            } else if maj >= 10.0 {
                9.5
            } else if maj >= 3.0 {
                11.5
            } else {
                14.5
            }
        };

        if self.is_messier() { raw.min(9.5) } else { raw }
    }
}

/// Decoded Apogee Deep-Sky Object Catalog.
#[derive(Clone, Debug, PartialEq)]
pub struct DsoCatalog {
    pub header: DsoCatalogHeader,
    pub records: Vec<DsoRecord>,
}

fn read_u8(reader: &mut impl Read) -> Result<u8, io::Error> {
    let mut buf = [0u8; 1];
    reader.read_exact(&mut buf)?;
    Ok(buf[0])
}

fn read_u16_le(reader: &mut impl Read) -> Result<u16, io::Error> {
    let mut buf = [0u8; 2];
    reader.read_exact(&mut buf)?;
    Ok(u16::from_le_bytes(buf))
}

fn read_f32_le(reader: &mut impl Read) -> Result<f32, io::Error> {
    let mut buf = [0u8; 4];
    reader.read_exact(&mut buf)?;
    Ok(f32::from_le_bytes(buf))
}

fn read_f64_le(reader: &mut impl Read) -> Result<f64, io::Error> {
    let mut buf = [0u8; 8];
    reader.read_exact(&mut buf)?;
    Ok(f64::from_le_bytes(buf))
}

fn read_short_string(reader: &mut impl Read) -> Result<String, io::Error> {
    let len = read_u8(reader)? as usize;
    let mut bytes = vec![0u8; len];
    reader.read_exact(&mut bytes)?;
    String::from_utf8(bytes).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Invalid UTF-8 in DSO catalog string: {e}"),
        )
    })
}

fn read_short_string_list(reader: &mut impl Read) -> Result<Vec<String>, io::Error> {
    let count = read_u8(reader)? as usize;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        out.push(read_short_string(reader)?);
    }
    Ok(out)
}

fn write_short_string(writer: &mut impl Write, s: &str) -> Result<(), io::Error> {
    let len = u8::try_from(s.len()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("DSO catalog string exceeds 255 bytes: {} bytes", s.len()),
        )
    })?;
    writer.write_all(&[len])?;
    writer.write_all(s.as_bytes())
}

fn write_short_string_list(writer: &mut impl Write, list: &[String]) -> Result<(), io::Error> {
    let count = u8::try_from(list.len()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("DSO catalog list exceeds 255 items: {}", list.len()),
        )
    })?;
    writer.write_all(&[count])?;
    for item in list {
        write_short_string(writer, item)?;
    }
    Ok(())
}

/// Reads a zstd-compressed Apogee deep-sky object catalog (`dso.bin.zst`).
pub fn read_dso_catalog_from_zst(reader: impl Read) -> Result<DsoCatalog, io::Error> {
    let mut decoder = zstd::stream::Decoder::new(reader)?;
    let mut header = DsoCatalogHeader::zeroed();
    decoder.read_exact(bytemuck::bytes_of_mut(&mut header))?;
    header.validate()?;

    let count = header.object_count as usize;
    let mut records = Vec::with_capacity(count);

    for idx in 0..count {
        let flags = read_u16_le(&mut decoder)?;
        if flags & !FLAG_ALL != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Unsupported DSO flags {flags:#06x} at record {idx}"),
            ));
        }

        let ra_deg = read_f64_le(&mut decoder)?;
        let dec_deg = read_f64_le(&mut decoder)?;
        if !ra_deg.is_finite() || !dec_deg.is_finite() || !(-90.0..=90.0).contains(&dec_deg) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Invalid coordinates ({ra_deg}, {dec_deg}) at DSO record {idx}"),
            ));
        }

        let kind_raw = read_u8(&mut decoder)?;
        let kind = DsoKind::from_u8(kind_raw).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Unknown DSO kind {kind_raw} at record {idx}"),
            )
        })?;

        let mut constellation = [0u8; 3];
        decoder.read_exact(&mut constellation)?;
        if constellation != [0, 0, 0] && std::str::from_utf8(&constellation).is_err() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Invalid UTF-8 constellation code at DSO record {idx}"),
            ));
        }

        let messier = if flags & FLAG_MESSIER != 0 {
            Some(read_u16_le(&mut decoder)?)
        } else {
            None
        };
        let ngc = if flags & FLAG_NGC != 0 {
            Some(read_short_string(&mut decoder)?)
        } else {
            None
        };
        let ic = if flags & FLAG_IC != 0 {
            Some(read_short_string(&mut decoder)?)
        } else {
            None
        };
        let alt_ngc = if flags & FLAG_ALT_NGC != 0 {
            read_short_string_list(&mut decoder)?
        } else {
            Vec::new()
        };
        let common_names = if flags & FLAG_COMMON_NAMES != 0 {
            read_short_string_list(&mut decoder)?
        } else {
            Vec::new()
        };
        let v_mag = if flags & FLAG_V_MAG != 0 {
            Some(read_f32_le(&mut decoder)?)
        } else {
            None
        };
        let b_mag = if flags & FLAG_B_MAG != 0 {
            Some(read_f32_le(&mut decoder)?)
        } else {
            None
        };
        let surface_brightness = if flags & FLAG_SURFACE_BRIGHTNESS != 0 {
            Some(read_f32_le(&mut decoder)?)
        } else {
            None
        };
        let major_arcmin = if flags & FLAG_MAJOR_ARCMIN != 0 {
            Some(read_f32_le(&mut decoder)?)
        } else {
            None
        };
        let minor_arcmin = if flags & FLAG_MINOR_ARCMIN != 0 {
            Some(read_f32_le(&mut decoder)?)
        } else {
            None
        };
        let position_angle_deg = if flags & FLAG_POSITION_ANGLE != 0 {
            Some(read_f32_le(&mut decoder)?)
        } else {
            None
        };
        let hubble_type = if flags & FLAG_HUBBLE_TYPE != 0 {
            Some(read_short_string(&mut decoder)?)
        } else {
            None
        };

        records.push(DsoRecord {
            ra_deg: ra_deg.rem_euclid(360.0),
            dec_deg,
            kind,
            constellation,
            messier,
            ngc,
            ic,
            alt_ngc,
            common_names,
            v_mag,
            b_mag,
            surface_brightness,
            major_arcmin,
            minor_arcmin,
            position_angle_deg,
            hubble_type,
        });
    }

    Ok(DsoCatalog { header, records })
}

/// Writes a zstd-compressed Apogee deep-sky object catalog (`dso.bin.zst`).
pub fn write_dso_catalog_to_zst(
    writer: impl Write,
    catalog: &DsoCatalog,
    compression_level: i32,
) -> Result<(), io::Error> {
    catalog.header.validate()?;
    if catalog.header.object_count as usize != catalog.records.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "DSO header count ({}) does not match records length ({})",
                catalog.header.object_count,
                catalog.records.len()
            ),
        ));
    }

    let mut encoder = zstd::stream::Encoder::new(writer, compression_level)?;
    encoder.write_all(bytemuck::bytes_of(&catalog.header))?;

    for rec in &catalog.records {
        if !rec.ra_deg.is_finite()
            || !rec.dec_deg.is_finite()
            || !(-90.0..=90.0).contains(&rec.dec_deg)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("Invalid DSO coordinates ({}, {})", rec.ra_deg, rec.dec_deg),
            ));
        }

        let ngc = non_empty(rec.ngc.as_deref());
        let ic = non_empty(rec.ic.as_deref());
        let hubble = non_empty(rec.hubble_type.as_deref());
        let alt_ngc: Vec<String> = rec
            .alt_ngc
            .iter()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(ToString::to_string)
            .collect();
        let common_names: Vec<String> = rec
            .common_names
            .iter()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(ToString::to_string)
            .collect();

        let mut flags = 0_u16;
        if rec.messier.is_some() {
            flags |= FLAG_MESSIER;
        }
        if ngc.is_some() {
            flags |= FLAG_NGC;
        }
        if ic.is_some() {
            flags |= FLAG_IC;
        }
        if !alt_ngc.is_empty() {
            flags |= FLAG_ALT_NGC;
        }
        if !common_names.is_empty() {
            flags |= FLAG_COMMON_NAMES;
        }
        if rec.v_mag.is_some_and(f32::is_finite) {
            flags |= FLAG_V_MAG;
        }
        if rec.b_mag.is_some_and(f32::is_finite) {
            flags |= FLAG_B_MAG;
        }
        if rec.surface_brightness.is_some_and(f32::is_finite) {
            flags |= FLAG_SURFACE_BRIGHTNESS;
        }
        if rec.major_arcmin.is_some_and(f32::is_finite) {
            flags |= FLAG_MAJOR_ARCMIN;
        }
        if rec.minor_arcmin.is_some_and(f32::is_finite) {
            flags |= FLAG_MINOR_ARCMIN;
        }
        if rec.position_angle_deg.is_some_and(f32::is_finite) {
            flags |= FLAG_POSITION_ANGLE;
        }
        if hubble.is_some() {
            flags |= FLAG_HUBBLE_TYPE;
        }

        encoder.write_all(&flags.to_le_bytes())?;
        encoder.write_all(&rec.ra_deg.rem_euclid(360.0).to_le_bytes())?;
        encoder.write_all(&rec.dec_deg.to_le_bytes())?;
        encoder.write_all(&[rec.kind as u8])?;
        encoder.write_all(&rec.constellation)?;

        if let Some(m) = rec.messier {
            encoder.write_all(&m.to_le_bytes())?;
        }
        if let Some(s) = ngc {
            write_short_string(&mut encoder, s)?;
        }
        if let Some(s) = ic {
            write_short_string(&mut encoder, s)?;
        }
        if !alt_ngc.is_empty() {
            write_short_string_list(&mut encoder, &alt_ngc)?;
        }
        if !common_names.is_empty() {
            write_short_string_list(&mut encoder, &common_names)?;
        }
        if let Some(v) = rec.v_mag.filter(|v| v.is_finite()) {
            encoder.write_all(&v.to_le_bytes())?;
        }
        if let Some(b) = rec.b_mag.filter(|v| v.is_finite()) {
            encoder.write_all(&b.to_le_bytes())?;
        }
        if let Some(sb) = rec.surface_brightness.filter(|v| v.is_finite()) {
            encoder.write_all(&sb.to_le_bytes())?;
        }
        if let Some(maj) = rec.major_arcmin.filter(|v| v.is_finite()) {
            encoder.write_all(&maj.to_le_bytes())?;
        }
        if let Some(min) = rec.minor_arcmin.filter(|v| v.is_finite()) {
            encoder.write_all(&min.to_le_bytes())?;
        }
        if let Some(pa) = rec.position_angle_deg.filter(|v| v.is_finite()) {
            encoder.write_all(&pa.to_le_bytes())?;
        }
        if let Some(h) = hubble {
            write_short_string(&mut encoder, h)?;
        }
    }

    encoder.finish()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_m31() -> DsoRecord {
        DsoRecord {
            ra_deg: 10.684_79,
            dec_deg: 41.269_06,
            kind: DsoKind::Galaxy,
            constellation: *b"And",
            messier: Some(31),
            ngc: Some("224".to_string()),
            ic: None,
            alt_ngc: Vec::new(),
            common_names: vec!["Andromeda Galaxy".to_string()],
            v_mag: Some(3.44),
            b_mag: Some(4.29),
            surface_brightness: Some(23.58),
            major_arcmin: Some(177.83),
            minor_arcmin: Some(69.66),
            position_angle_deg: Some(35.0),
            hubble_type: Some("Sb".to_string()),
        }
    }

    fn sample_rosette_nebula() -> DsoRecord {
        DsoRecord {
            ra_deg: 97.98,
            dec_deg: 4.94,
            kind: DsoKind::HiiRegion,
            constellation: *b"Mon",
            messier: None,
            ngc: Some("2237".to_string()),
            ic: None,
            alt_ngc: vec!["2238".to_string(), "2246".to_string()],
            common_names: vec!["Rosette Nebula".to_string()],
            v_mag: None,
            b_mag: None,
            surface_brightness: None,
            major_arcmin: Some(80.0),
            minor_arcmin: Some(60.0),
            position_angle_deg: None,
            hubble_type: None,
        }
    }

    #[test]
    fn dso_catalog_roundtrip_and_header_validation() {
        let records = vec![sample_m31(), sample_rosette_nebula()];
        let catalog = DsoCatalog {
            header: DsoCatalogHeader::new(2, 1, 2),
            records: records.clone(),
        };

        let mut buf = Vec::new();
        write_dso_catalog_to_zst(&mut buf, &catalog, 3).expect("write dso catalog");
        let decoded = read_dso_catalog_from_zst(buf.as_slice()).expect("read dso catalog");
        assert_eq!(decoded, catalog);

        let bad_magic = DsoCatalogHeader {
            magic: *b"BAD!",
            ..catalog.header
        };
        assert!(bad_magic.validate().is_err());

        let bad_version = DsoCatalogHeader {
            version: DSO_CATALOG_VERSION + 1,
            ..catalog.header
        };
        assert!(bad_version.validate().is_err());
    }

    #[test]
    fn dso_display_formatting_and_effective_mag() {
        let m31 = sample_m31();
        assert_eq!(m31.primary_designation(), "M31");
        assert_eq!(m31.hover_title(), "M31 · Andromeda Galaxy");
        assert_eq!(
            m31.display_title_and_subtitle(),
            (
                "M31".to_string(),
                Some("Andromeda Galaxy · NGC 224 · Sb · And".to_string())
            )
        );
        assert_eq!(m31.catalog_ids_summary().as_deref(), Some("M31 · NGC 224"));
        assert_eq!(m31.mag_label(), "V 3.4");
        assert_eq!(m31.mag_detail_label(), "V 3.44");
        assert_eq!(m31.angular_size_label().as_deref(), Some("177.8' × 69.7'"));
        assert!((m31.effective_display_mag() - 3.44).abs() < 1e-5);

        let rosette = sample_rosette_nebula();
        assert_eq!(rosette.primary_designation(), "NGC 2237");
        assert_eq!(
            rosette.catalog_ids_summary().as_deref(),
            Some("NGC 2237 · NGC 2238 · NGC 2246")
        );
        assert_eq!(rosette.mag_label(), "—");
        assert!((rosette.effective_display_mag() - 6.5).abs() < 1e-5);

        let tiny_pn = DsoRecord {
            ra_deg: 283.396,
            dec_deg: 33.028,
            kind: DsoKind::PlanetaryNebula,
            constellation: *b"Lyr",
            messier: Some(57),
            ngc: Some("6720".to_string()),
            ic: None,
            alt_ngc: Vec::new(),
            common_names: vec!["Ring Nebula".to_string()],
            v_mag: Some(8.8),
            b_mag: Some(9.7),
            surface_brightness: None,
            major_arcmin: Some(1.27),
            minor_arcmin: Some(1.25),
            position_angle_deg: None,
            hubble_type: None,
        };
        // Minor within 5% of major collapses to single dimension.
        assert_eq!(tiny_pn.angular_size_label().as_deref(), Some("1.3'"));

        let arcsec_pn = DsoRecord {
            major_arcmin: Some(0.35),
            minor_arcmin: None,
            ..tiny_pn
        };
        assert_eq!(arcsec_pn.angular_size_label().as_deref(), Some("21\""));

        let faint_messier = DsoRecord {
            v_mag: Some(10.2),
            ..sample_m31()
        };
        assert!((faint_messier.effective_display_mag() - 9.5).abs() < 1e-5);
    }

    #[test]
    fn dso_kind_mappings_cover_all_variants() {
        for raw in 0..=17_u8 {
            let kind = DsoKind::from_u8(raw).expect("valid DsoKind");
            assert_eq!(kind as u8, raw);
            assert_ne!(kind.label(), "");
            assert_ne!(kind.short_label(), "");
            assert!(kind.symbol_code() <= 7);
            assert!(kind.color_id() <= 8);
        }
        assert!(DsoKind::from_u8(18).is_none());

        assert_eq!(DsoKind::from_openngc_code("G"), Some(DsoKind::Galaxy));
        assert_eq!(
            DsoKind::from_openngc_code("Cl+N"),
            Some(DsoKind::ClusterNebula)
        );
        assert_eq!(
            DsoKind::from_openngc_code("*Ass"),
            Some(DsoKind::StarAssociation)
        );
        assert_eq!(DsoKind::from_openngc_code("**"), Some(DsoKind::DoubleStar));
        assert_eq!(
            DsoKind::from_openngc_code("SNR"),
            Some(DsoKind::SupernovaRemnant)
        );
    }
}

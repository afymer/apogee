use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use crate::grid::HipsGrid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TileFormat {
    Jpg,
    Png,
}

impl TileFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Jpg => "jpg",
            Self::Png => "png",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HipsSurvey {
    pub name: String,
    pub base_url: String,
    pub max_depth: u8,
    pub format: TileFormat,
}

impl HipsSurvey {
    pub fn dss_color() -> Self {
        Self {
            name: "CDS DSS Color".into(),
            base_url: "https://alasky.cds.unistra.fr/DSS/DSSColor".into(),
            max_depth: 9,
            format: TileFormat::Jpg,
        }
    }

    /// URL for a specific tile according to the HiPS standard:
    /// `{base_url}/Norder{depth}/Dir{dir}/Npix{hash}.{ext}`
    /// where `dir = (hash / 10000) * 10000`.
    pub fn tile_url(&self, depth: u8, hash: u64) -> String {
        let dir = (hash / 10000) * 10000;
        format!(
            "{}/Norder{depth}/Dir{dir}/Npix{hash}.{}",
            self.base_url.trim_end_matches('/'),
            self.format.extension()
        )
    }

    pub fn tile_relative_path(&self, depth: u8, hash: u64) -> PathBuf {
        let dir = (hash / 10000) * 10000;
        PathBuf::from(format!(
            "Norder{depth}/Dir{dir}/Npix{hash}.{}",
            self.format.extension()
        ))
    }

    pub fn slug(&self) -> String {
        self.name
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '_' })
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TileMeshGeometry {
    pub positions: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

impl HipsGrid {
    pub fn default_tile_subdivisions(depth: u8) -> usize {
        match depth {
            0 => 8,
            1..=2 => 4,
            _ => 2,
        }
    }

    #[allow(clippy::many_single_char_names)]
    pub fn tile_surface_mesh(depth: u8, hash: u64, subdivisions: usize) -> TileMeshGeometry {
        let m = subdivisions.max(1);
        let layer = cdshealpix::nested::get(depth);
        let vertex_count = (m + 1) * (m + 1);
        let mut positions = Vec::with_capacity(vertex_count);
        let mut uvs = Vec::with_capacity(vertex_count);
        let mut indices = Vec::with_capacity(m * m * 6);

        for j in 0..=m {
            for i in 0..=m {
                let u = i as f64 / m as f64;
                let v = j as f64 / m as f64;
                let dx = v.clamp(1e-7, 1.0 - 1e-7);
                let dy = u.clamp(1e-7, 1.0 - 1e-7);
                let (lon, lat) = layer.sph_coo(hash, dx, dy);
                let cos_lat = lat.cos();
                let x = (cos_lat * lon.cos()) as f32;
                let y = (cos_lat * lon.sin()) as f32;
                let z = lat.sin() as f32;
                positions.push([x, y, z]);
                uvs.push([u as f32, v as f32]);
            }
        }

        for j in 0..m {
            for i in 0..m {
                let stride = (m + 1) as u32;
                let i0 = j as u32 * stride + i as u32;
                let i1 = i0 + 1;
                let i2 = (j as u32 + 1) * stride + (i as u32 + 1);
                let i3 = (j as u32 + 1) * stride + i as u32;

                indices.push(i0);
                indices.push(i3);
                indices.push(i1);

                indices.push(i1);
                indices.push(i3);
                indices.push(i2);
            }
        }

        TileMeshGeometry {
            positions,
            uvs,
            indices,
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum TileFetchError {
    #[error("HTTP request error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Tile not found: HTTP {0}")]
    NotFound(u16),
    #[error("Server returned HTTP {0}")]
    HttpStatus(u16),
}

#[derive(Clone)]
pub struct HipsTileFetcher {
    client: reqwest::Client,
    cache_dir: Option<PathBuf>,
}

impl HipsTileFetcher {
    pub fn new(cache_dir: Option<PathBuf>) -> Self {
        let client = reqwest::Client::builder()
            .user_agent("ApogeePlanetarium/0.1.0")
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(20))
            .build()
            .unwrap_or_default();
        Self { client, cache_dir }
    }

    pub fn default_cache_root() -> Option<PathBuf> {
        let base = std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
        Some(base.join("apogee").join("hips"))
    }

    pub fn tile_cache_path(&self, survey: &HipsSurvey, depth: u8, hash: u64) -> Option<PathBuf> {
        self.cache_dir.as_ref().map(|root| {
            root.join(survey.slug())
                .join(survey.tile_relative_path(depth, hash))
        })
    }

    pub async fn fetch_tile(
        &self,
        survey: &HipsSurvey,
        depth: u8,
        hash: u64,
    ) -> Result<Vec<u8>, TileFetchError> {
        static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

        if let Some(path) = self.tile_cache_path(survey, depth, hash)
            && let Ok(bytes) = tokio::fs::read(&path).await
        {
            return Ok(bytes);
        }

        let url = survey.tile_url(depth, hash);
        let resp = self.client.get(&url).send().await?;
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(TileFetchError::NotFound(status.as_u16()));
        }
        if !status.is_success() {
            return Err(TileFetchError::HttpStatus(status.as_u16()));
        }

        let bytes = resp.bytes().await?.to_vec();

        if let Some(path) = self.tile_cache_path(survey, depth, hash) {
            if let Some(parent) = path.parent() {
                let _ = tokio::fs::create_dir_all(parent).await;
            }
            let seq = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let tmp_path = path.with_extension(format!("tmp.{}.{seq}", std::process::id()));
            if tokio::fs::write(&tmp_path, &bytes).await.is_ok()
                && tokio::fs::rename(&tmp_path, &path).await.is_err()
            {
                let _ = tokio::fs::remove_file(&tmp_path).await;
            }
        }

        Ok(bytes)
    }
}

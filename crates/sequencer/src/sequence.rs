use enumflags2::{BitFlags, bitflags};
use std::time::Duration;

#[bitflags]
#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum SaveLocation {
    Camera,
    Server,
    Viewer,
}

#[derive(Clone, Debug)]
pub struct SaveInfo {
    #[allow(dead_code)]
    save_locations: BitFlags<SaveLocation>,
    #[allow(dead_code)]
    path: String,
}

#[derive(Clone, Debug)]
pub struct Frame {
    #[allow(dead_code)]
    exposure_time: Duration,
    #[allow(dead_code)]
    save_info: SaveInfo,
}

#[derive(Clone, Debug)]
pub struct Sequence {
    #[allow(dead_code)]
    frames: Vec<Frame>,
}

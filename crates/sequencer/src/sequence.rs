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

#[derive(Clone)]
pub struct SaveInfo {
    save_locations: BitFlags<SaveLocation>,
    path: String,
}

#[derive(Clone)]
pub struct Frame {
    exposure_time: Duration,
    save_info: SaveInfo,
}

#[derive(Clone)]
pub struct Sequence {
    frames: Vec<Frame>,
}

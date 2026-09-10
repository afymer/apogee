pub mod sequence;

use gphoto::handle::CameraHandle;

pub struct Sequencer {
    #[allow(dead_code)]
    camera: CameraHandle,
}

impl Sequencer {
    pub fn new(camera: CameraHandle) -> Self {
        Self { camera }
    }
}

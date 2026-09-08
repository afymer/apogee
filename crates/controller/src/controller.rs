pub mod controller_handle;
pub mod controller_job;

use gphoto::handle::CameraHandle;
use sequencer::Sequencer;

use crate::controller_handle::ControllerHandle;

pub struct Controller {
    sequencer: Sequencer,
}

impl Controller {
    fn new(camera: CameraHandle) -> Self {
        let sequencer = Sequencer::new(camera);
        Self { sequencer }
    }

    pub fn spawn(camera: CameraHandle) -> ControllerHandle {
        let handle = ControllerHandle::new();

        let controller = Self::new(camera);

        tokio::spawn(async move { controller.run().await });

        handle
    }

    async fn run(mut self) {}
}

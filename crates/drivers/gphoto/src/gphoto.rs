pub mod actor;
pub mod handle;
pub mod types;

pub trait DriverState {}

pub struct Uninitialized;
impl DriverState for Uninitialized {}

pub struct Initialized {
    camera: gphoto2::Camera,
}
impl DriverState for Initialized {}

pub struct Driver<S: DriverState> {
    state: S,
}

impl Default for Driver<Uninitialized> {
    fn default() -> Self {
        Self {
            state: Uninitialized,
        }
    }
}

impl Driver<Uninitialized> {
    pub async fn initialize(
        self,
        context: &gphoto2::Context,
    ) -> Result<Driver<Initialized>, gphoto2::Error> {
        let camera = context.autodetect_camera().await?;

        Ok(Driver {
            state: Initialized { camera },
        })
    }
}

impl Driver<Initialized> {
    pub fn camera(&self) -> &gphoto2::Camera {
        &self.state.camera
    }

    pub async fn wait_event(
        &self,
        timeout: std::time::Duration,
    ) -> Result<gphoto2::camera::CameraEvent, gphoto2::Error> {
        self.state.camera.wait_event(timeout).await
    }
}

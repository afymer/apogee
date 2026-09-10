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

    pub async fn keep_alive(&self) -> Result<(), gphoto2::Error> {
        self.state.camera.storages().await.map(|_| ())
    }

    pub async fn wait_event(
        &self,
        timeout: std::time::Duration,
    ) -> Result<gphoto2::camera::CameraEvent, gphoto2::Error> {
        self.state.camera.wait_event(timeout).await
    }

    pub fn close(self) {
        // Because the doc of from_ref states that
        // the recommended way to write this code is
        // to avoid relying on lifetime extension
        // when raw pointers are involved
        let ptr = self.state.camera.as_ref();
        let cam_ptr = std::ptr::from_ref(ptr).cast_mut();
        unsafe {
            let _ = gphoto2::libgphoto2_sys::gp_camera_exit(cam_ptr, std::ptr::null_mut());
        }
    }
}

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

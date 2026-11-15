use protocol::Frame;

#[derive(Clone, Debug)]
pub enum ControllerJob {
    Capture(Frame),
}

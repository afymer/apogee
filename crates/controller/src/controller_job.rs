use protocol::{Frame, ImagingSequence};

#[derive(Clone, Debug)]
pub enum ControllerJob {
    Capture(Frame),
    Sequence(ImagingSequence),
}

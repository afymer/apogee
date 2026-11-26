use protocol::{Frame, ImagingSequence, NightPlanRunRequest};

#[derive(Clone, Debug)]
pub enum ControllerJob {
    Capture(Frame),
    Sequence(ImagingSequence),
    NightPlan(NightPlanRunRequest),
}

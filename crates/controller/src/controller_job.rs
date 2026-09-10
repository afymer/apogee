use protocol::Frame;
use sequencer::sequence::Sequence;

#[derive(Clone, Debug)]
pub enum ControllerJob {
    Capture(Frame),
    RunSequence(Sequence),
}

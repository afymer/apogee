use sequencer::sequence::Sequence;

#[derive(Clone)]
enum ControllerJob {
    RunSequence(Sequence),
}

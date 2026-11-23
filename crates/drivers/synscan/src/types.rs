use thiserror::Error;

#[derive(Clone, Debug)]
pub enum MountEvent {
    Ports(Vec<String>),
    Error(String),
}

#[derive(Debug, Error)]
pub enum MountError {
    #[error("Mount actor disconnected")]
    ActorDisconnected,
    #[error("Mount is not connected")]
    NotConnected,
    #[error("Mount is parked; unpark before slewing")]
    Parked,
    #[error("Serial I/O error: {0}")]
    Io(String),
    #[error("Protocol error: {0}")]
    Protocol(String),
    #[error("Limit violation: {0}")]
    LimitViolation(String),
    #[error("Unsupported platform for serial port access")]
    UnsupportedPlatform,
}

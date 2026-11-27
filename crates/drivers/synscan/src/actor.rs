use std::time::{Duration, Instant};

use protocol::{
    MountCommand, MountDriverKind, MountLimits, MountStatus, MountTelemetry, TrackingRate,
};
use tokio::sync::{broadcast, mpsc, watch};
use tracing::{info, warn};

use crate::coords::{DEFAULT_SITE_LAT_DEG, DEFAULT_SITE_LON_DEG};
use crate::handle::MountHandle;
use crate::serial::{SynScanSerialPort, list_serial_ports};
use crate::serial_session::{HandshakeInfo, SerialSession, handshake_transport};
use crate::simulator::SimulatedMount;
use crate::types::{MountError, MountEvent};

enum ActiveBackend {
    None,
    Simulator(SimulatedMount),
    Serial(SerialSession),
}

pub struct MountActor {
    cmd_rx: mpsc::Receiver<MountCommand>,
    status_tx: watch::Sender<MountStatus>,
    telemetry_tx: watch::Sender<Option<MountTelemetry>>,
    event_tx: broadcast::Sender<MountEvent>,
    backend: ActiveBackend,
    site_lat_deg: f64,
    site_lon_deg: f64,
    limits: MountLimits,
    last_tick: Instant,
    consecutive_poll_errors: u32,
}

impl MountActor {
    pub fn spawn() -> MountHandle {
        Self::spawn_inner(false)
    }

    /// Spawns a mount actor pre-connected to the built-in NEQ6-Pro simulator.
    pub fn spawn_simulated() -> MountHandle {
        Self::spawn_inner(true)
    }

    fn spawn_inner(start_simulated: bool) -> MountHandle {
        let (cmd_tx, cmd_rx) = mpsc::channel(64);
        let (initial_status, initial_telemetry, initial_backend) = if start_simulated {
            let mut sim = SimulatedMount::default();
            sim.unpark();
            sim.tracking = TrackingRate::Sidereal;
            let telem = sim.telemetry();
            (
                MountStatus::Ready,
                Some(telem),
                ActiveBackend::Simulator(sim),
            )
        } else {
            (MountStatus::Disconnected, None, ActiveBackend::None)
        };

        let (status_tx, status_rx) = watch::channel(initial_status);
        let (telemetry_tx, telemetry_rx) = watch::channel(initial_telemetry);
        let (event_tx, _) = broadcast::channel(64);

        let handle = MountHandle::new(cmd_tx, status_rx, telemetry_rx, event_tx.clone());

        let actor = Self {
            cmd_rx,
            status_tx,
            telemetry_tx,
            event_tx,
            backend: initial_backend,
            site_lat_deg: DEFAULT_SITE_LAT_DEG,
            site_lon_deg: DEFAULT_SITE_LON_DEG,
            limits: MountLimits::default(),
            last_tick: Instant::now(),
            consecutive_poll_errors: 0,
        };

        tokio::spawn(async move { actor.run().await });
        handle
    }

    async fn run(mut self) {
        let mut poll_interval = tokio::time::interval(Duration::from_millis(150));
        poll_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                biased;
                cmd = self.cmd_rx.recv() => {
                    match cmd {
                        Some(cmd) => self.handle_command(cmd).await,
                        None => {
                            info!("MountActor command channel closed, shutting down");
                            break;
                        }
                    }
                }
                _ = poll_interval.tick() => {
                    self.poll_backend().await;
                }
            }
        }
    }

    fn set_status(&self, status: MountStatus) {
        if *self.status_tx.borrow() != status {
            let _ = self.status_tx.send(status);
        }
    }

    fn emit_error(&self, msg: impl Into<String>) {
        let msg = msg.into();
        warn!("Mount error: {msg}");
        let _ = self.event_tx.send(MountEvent::Error(msg));
    }

    fn report_result(&self, context: &str, res: Result<(), MountError>) {
        if let Err(err) = res {
            self.emit_error(if context.is_empty() {
                err.to_string()
            } else {
                format!("{context}: {err}")
            });
        }
    }

    async fn handle_command(&mut self, cmd: MountCommand) {
        match cmd {
            MountCommand::ListPorts => {
                let ports = tokio::task::spawn_blocking(list_serial_ports)
                    .await
                    .unwrap_or_default();
                let _ = self.event_tx.send(MountEvent::Ports(ports));
            }
            MountCommand::Connect {
                driver,
                port,
                baud_rate,
            } => {
                self.connect(driver, port, baud_rate).await;
            }
            MountCommand::Disconnect => {
                self.backend = ActiveBackend::None;
                self.consecutive_poll_errors = 0;
                self.set_status(MountStatus::Disconnected);
                let _ = self.telemetry_tx.send(None);
                info!("Mount disconnected");
            }
            MountCommand::SetLimits(mut limits) => {
                limits.sanitize();
                self.limits = limits;
                match &mut self.backend {
                    ActiveBackend::Simulator(sim) => sim.limits = limits,
                    ActiveBackend::Serial(sess) => sess.limits = limits,
                    ActiveBackend::None => {}
                }
                self.poll_backend().await;
            }
            MountCommand::SetSite {
                latitude_deg,
                longitude_deg,
                elevation_m: _,
            } => {
                if !latitude_deg.is_finite() || !longitude_deg.is_finite() {
                    self.emit_error("Site coordinates must be finite");
                    return;
                }
                self.site_lat_deg = latitude_deg.clamp(-90.0, 90.0);
                self.site_lon_deg = longitude_deg.clamp(-180.0, 180.0);
                let res = match &mut self.backend {
                    ActiveBackend::Simulator(sim) => {
                        sim.set_site(self.site_lat_deg, self.site_lon_deg);
                        Ok(())
                    }
                    ActiveBackend::Serial(sess) => {
                        sess.set_site(self.site_lat_deg, self.site_lon_deg).await
                    }
                    ActiveBackend::None => Ok(()),
                };
                self.report_result("Failed to sync site to SynScan", res);
                self.poll_backend().await;
            }
            MountCommand::SlewToEquatorial { ra_deg, dec_deg } => {
                let res = match &mut self.backend {
                    ActiveBackend::None => Err(MountError::NotConnected),
                    ActiveBackend::Simulator(sim) => sim.goto_equatorial(ra_deg, dec_deg),
                    ActiveBackend::Serial(sess) => sess.slew_equatorial(ra_deg, dec_deg).await,
                };
                self.report_result("", res);
            }
            MountCommand::SlewToHorizontal { az_deg, alt_deg } => {
                let res = match &mut self.backend {
                    ActiveBackend::None => Err(MountError::NotConnected),
                    ActiveBackend::Simulator(sim) => sim.goto_horizontal(az_deg, alt_deg),
                    ActiveBackend::Serial(sess) => sess.slew_horizontal(az_deg, alt_deg).await,
                };
                self.report_result("", res);
            }
            MountCommand::SyncEquatorial { ra_deg, dec_deg } => {
                let res = match &mut self.backend {
                    ActiveBackend::None => Err(MountError::NotConnected),
                    ActiveBackend::Simulator(sim) => sim.sync_equatorial(ra_deg, dec_deg),
                    ActiveBackend::Serial(sess) => sess.sync_equatorial(ra_deg, dec_deg).await,
                };
                self.report_result("Sync RA/Dec failed", res);
            }
            MountCommand::StartManualSlew { direction, rate } => {
                let res = match &mut self.backend {
                    ActiveBackend::None => Err(MountError::NotConnected),
                    ActiveBackend::Simulator(sim) => sim.start_manual_slew(direction, rate),
                    ActiveBackend::Serial(sess) => sess.start_manual_slew(direction, rate).await,
                };
                self.report_result("", res);
            }
            MountCommand::StopManualSlew { axis } => {
                let res = match &mut self.backend {
                    ActiveBackend::None => Ok(()),
                    ActiveBackend::Simulator(sim) => {
                        sim.stop_manual_slew(axis);
                        Ok(())
                    }
                    ActiveBackend::Serial(sess) => sess.stop_manual_slew(axis).await,
                };
                self.report_result("Stop axis failed", res);
            }
            MountCommand::StopAll => {
                let res = match &mut self.backend {
                    ActiveBackend::None => Ok(()),
                    ActiveBackend::Simulator(sim) => {
                        sim.stop_all();
                        Ok(())
                    }
                    ActiveBackend::Serial(sess) => sess.stop_all().await,
                };
                self.report_result("Emergency stop failed", res);
            }
            MountCommand::SetTracking(rate) => {
                let res = match &mut self.backend {
                    ActiveBackend::None => Err(MountError::NotConnected),
                    ActiveBackend::Simulator(sim) => sim.set_tracking(rate),
                    ActiveBackend::Serial(sess) => sess.set_tracking(rate).await,
                };
                self.report_result("Set tracking failed", res);
            }
            MountCommand::SetSlewRate(rate) => {
                let clamped = rate.clamp(1, 9);
                match &mut self.backend {
                    ActiveBackend::Simulator(sim) => sim.slew_rate = clamped,
                    ActiveBackend::Serial(sess) => sess.slew_rate = clamped,
                    ActiveBackend::None => {}
                }
                self.poll_backend().await;
            }
            MountCommand::Park => {
                let res = match &mut self.backend {
                    ActiveBackend::None => Err(MountError::NotConnected),
                    ActiveBackend::Simulator(sim) => {
                        sim.park();
                        Ok(())
                    }
                    ActiveBackend::Serial(sess) => sess.park().await,
                };
                self.report_result("Park failed", res);
            }
            MountCommand::Unpark => {
                let res = match &mut self.backend {
                    ActiveBackend::None => Err(MountError::NotConnected),
                    ActiveBackend::Simulator(sim) => {
                        sim.unpark();
                        Ok(())
                    }
                    ActiveBackend::Serial(sess) => {
                        sess.unpark();
                        Ok(())
                    }
                };
                self.report_result("", res);
                self.poll_backend().await;
            }
            MountCommand::SetParkHere => {
                let current = self.telemetry_tx.borrow().clone();
                let res = match &mut self.backend {
                    ActiveBackend::None => Err(MountError::NotConnected),
                    ActiveBackend::Simulator(sim) => {
                        sim.set_park_here();
                        Ok(())
                    }
                    ActiveBackend::Serial(sess) => sess.set_park_here(current.as_ref()),
                };
                self.report_result("Set park position failed", res);
            }
            MountCommand::GoHome => {
                let res = match &mut self.backend {
                    ActiveBackend::None => Err(MountError::NotConnected),
                    ActiveBackend::Simulator(sim) => sim.go_home(),
                    ActiveBackend::Serial(sess) => sess.go_home().await,
                };
                self.report_result("Go Home failed", res);
            }
            MountCommand::MeridianFlip => {
                let current = self.telemetry_tx.borrow().clone();
                let res = match &mut self.backend {
                    ActiveBackend::None => Err(MountError::NotConnected),
                    ActiveBackend::Simulator(sim) => sim.meridian_flip(),
                    ActiveBackend::Serial(sess) => sess.meridian_flip(current.as_ref()).await,
                };
                self.report_result("Meridian flip failed", res);
            }
        }
    }

    async fn connect(&mut self, driver: MountDriverKind, port: String, baud_rate: u32) {
        self.backend = ActiveBackend::None;
        self.consecutive_poll_errors = 0;
        let display_port = if driver == MountDriverKind::Simulator {
            "simulator".to_string()
        } else {
            port.clone()
        };
        self.set_status(MountStatus::Connecting { port: display_port });

        match driver {
            MountDriverKind::Simulator => {
                let mut sim = SimulatedMount::default();
                sim.limits = self.limits;
                sim.set_site(self.site_lat_deg, self.site_lon_deg);
                let initial = sim.telemetry();
                self.backend = ActiveBackend::Simulator(sim);
                self.last_tick = Instant::now();
                self.set_status(MountStatus::Ready);
                let _ = self.telemetry_tx.send(Some(initial));
                info!("Connected to NEQ6-Pro Simulator");
            }
            MountDriverKind::SynScanSerial => {
                let baud = if baud_rate == 0 { 9600 } else { baud_rate };
                let path = port.clone();
                let open_res = tokio::task::spawn_blocking(
                    move || -> Result<(SynScanSerialPort, HandshakeInfo), MountError> {
                        let mut serial = SynScanSerialPort::open(&path, baud)?;
                        let info = handshake_transport(&mut serial)?;
                        Ok((serial, info))
                    },
                )
                .await;

                match open_res {
                    Ok(Ok((serial, info))) => {
                        info!(
                            "Connected to SynScan mount on {port} (model={:?}, fw={:?})",
                            info.mount_model, info.firmware_version
                        );
                        let session = SerialSession::new(
                            port,
                            Box::new(serial),
                            info,
                            self.site_lat_deg,
                            self.site_lon_deg,
                            self.limits,
                        );
                        self.backend = ActiveBackend::Serial(session);
                        self.set_status(MountStatus::Ready);
                        self.poll_backend().await;
                    }
                    Ok(Err(err)) => {
                        let msg = err.to_string();
                        self.set_status(MountStatus::Error(msg.clone()));
                        self.emit_error(msg);
                    }
                    Err(join_err) => {
                        let msg = format!("Serial connect task failed: {join_err}");
                        self.set_status(MountStatus::Error(msg.clone()));
                        self.emit_error(msg);
                    }
                }
            }
        }
    }

    async fn poll_backend(&mut self) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_tick).as_secs_f64();
        self.last_tick = now;

        match &mut self.backend {
            ActiveBackend::None => {}
            ActiveBackend::Simulator(sim) => {
                sim.step(dt);
                let telemetry = sim.telemetry();
                let _ = self.telemetry_tx.send(Some(telemetry));
            }
            ActiveBackend::Serial(sess) => match sess.poll().await {
                Ok(telemetry) => {
                    self.consecutive_poll_errors = 0;
                    self.set_status(MountStatus::Ready);
                    let _ = self.telemetry_tx.send(Some(telemetry));
                }
                Err(err) => {
                    self.consecutive_poll_errors = self.consecutive_poll_errors.saturating_add(1);
                    warn!(
                        "Serial poll error (#{}, err={err})",
                        self.consecutive_poll_errors
                    );
                    self.set_status(MountStatus::Error(err.to_string()));
                    // After 5 consecutive I/O failures (e.g. USB unplugged), drop the dead session
                    // instead of hammering the closed descriptor forever.
                    if self.consecutive_poll_errors >= 5 {
                        self.backend = ActiveBackend::None;
                        let _ = self.telemetry_tx.send(None);
                        self.emit_error(format!(
                            "Mount connection lost after repeated errors: {err}"
                        ));
                    }
                }
            },
        }
    }
}

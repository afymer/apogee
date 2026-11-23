//! Shared keyboard manual-slew controller for Mount panels.
//!
//! Handles arrow keys (N/S/E/W) and Space/Escape emergency stop with:
//! - Readiness & park guards (ignores slews while disconnected or parked)
//! - Proper opposing-key resolution on the same axis (releasing Up while Down is held
//!   continues slewing South instead of stopping Dec!)
//! - Clean `release_all` on focus blur / window deactivation / panel drop so the mount
//!   can never get stuck slewing indefinitely.

use client::send_mount_command;
use gpui::{App, KeyDownEvent, KeyUpEvent};
use protocol::{MountAxis, MountCommand, SlewDirection};

#[derive(Default)]
pub struct ManualSlewKeyController {
    /// `[North, South, East, West]`
    held: [bool; 4],
}

impl ManualSlewKeyController {
    fn key_to_index(key: &str) -> Option<(usize, SlewDirection)> {
        match key {
            "up" => Some((0, SlewDirection::North)),
            "down" => Some((1, SlewDirection::South)),
            "left" => Some((2, SlewDirection::East)),
            "right" => Some((3, SlewDirection::West)),
            _ => None,
        }
    }

    /// Handles a key-down event. Returns `true` if the event was consumed.
    pub fn handle_key_down(
        &mut self,
        ev: &KeyDownEvent,
        can_slew: bool,
        slew_rate: u8,
        cx: &App,
    ) -> bool {
        let key = ev.keystroke.key.as_str();
        if key == "space" || key == "escape" {
            self.held = [false; 4];
            send_mount_command(MountCommand::StopAll, cx);
            return true;
        }
        let Some((idx, dir)) = Self::key_to_index(key) else {
            return false;
        };
        if !can_slew {
            return true;
        }
        if !self.held[idx] {
            self.held[idx] = true;
            send_mount_command(
                MountCommand::StartManualSlew {
                    direction: dir,
                    rate: slew_rate,
                },
                cx,
            );
        }
        true
    }

    /// Handles a key-up event. If the opposing key on the same axis is still held,
    /// resumes slewing in that direction instead of stopping the axis.
    pub fn handle_key_up(&mut self, ev: &KeyUpEvent, slew_rate: u8, cx: &App) -> bool {
        let Some((idx, dir)) = Self::key_to_index(ev.keystroke.key.as_str()) else {
            return false;
        };
        if !self.held[idx] {
            return false;
        }
        self.held[idx] = false;

        let (opp_idx, opp_dir) = match idx {
            0 => (1, SlewDirection::South),
            1 => (0, SlewDirection::North),
            2 => (3, SlewDirection::West),
            _ => (2, SlewDirection::East),
        };

        if self.held[opp_idx] {
            send_mount_command(
                MountCommand::StartManualSlew {
                    direction: opp_dir,
                    rate: slew_rate,
                },
                cx,
            );
        } else {
            send_mount_command(MountCommand::StopManualSlew { axis: dir.axis() }, cx);
        }
        true
    }

    /// Stops any currently held manual-slew axes (call on blur / deactivation / drop).
    pub fn release_all(&mut self, cx: &App) {
        let dec_held = self.held[0] || self.held[1];
        let ra_held = self.held[2] || self.held[3];
        self.held = [false; 4];
        if dec_held {
            send_mount_command(MountCommand::StopManualSlew { axis: MountAxis::Dec }, cx);
        }
        if ra_held {
            send_mount_command(MountCommand::StopManualSlew { axis: MountAxis::Ra }, cx);
        }
    }

    pub fn is_any_held(&self) -> bool {
        self.held.iter().any(|&h| h)
    }
}

# This list evolves with each commit

## Features
- [mount/explore] Add J2000 <-> JNow precession conversion at the mount boundary
- [controller/capture] Implement multi-frame imaging Sequence Runner (`SequenceProgress`, pause/resume, auto-meridian-flip coordination between camera and mount)
- [planetarium] Add camera sensor FOV framing overlay (draggable/rotatable rectangle synced with mount GoTo)
- [mount_renderer] Optional animated 3D meridian flip (mechanical RA/Dec shaft angles or local pier-side transition animation)

## Architecture & Build Hygiene
- [apogee] Split `crates/apogee/src/bevy_viewport.rs` into focused modules (`mount_overlay.rs`, `trajectory_overlay.rs`, `picking.rs`)
- [build] Trim `tokio` features from `"full"` to the exact subset used, align `Cargo.cross.toml` with `Cargo.toml`, and pass `--locked` in `Dockerfile.cross`

## Rendering & UI
- [viewport] Partially move the HUD to the status bar
- [viewer] Bypass renderer scaling for the 1:1 mode
- [bevy] Make pipelined rendering work

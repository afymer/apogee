# Apogee

Astrophotography desktop app (GPUI + Bevy/wgpu) and Raspberry Pi hardware daemon (`daemon`) written in Rust (edition 2024).

## Workflow & Verification
- **VCS**: This repo uses Jujutsu (`jj`). Inspect changes with `jj status` / `jj diff`, commit with `jj describe` / `jj split` / `jj new` using conventional commits (`feat(scope): ...`, `fix(scope): ...`, `perf(scope): ...`).
- **Verify**: `cargo clippy --workspace --all-targets && cargo test --workspace` (workspace clippy sets `all = deny`, `pedantic = warn`).
- **Offline simulation**: Run `apogee` or `daemon` with `--sim` (or `APOGEE_SIM=1`) to use simulated camera and NEQ6-Pro mount backends without USB/serial hardware.
- **Raspberry Pi cross-build**: Uses `Cargo.cross.toml`, `Dockerfile.cross`, and `scripts/build-rpi.sh` targeting `crates/daemon` only. Keep daemon crates (`protocol`, `controller`, `drivers/*`, `daemon`) free of GPUI/Bevy dependencies.

## Architecture & Crate Boundaries
Keep heavy UI (`gpui`) and rendering (`bevy`/`planetarium`) dependencies isolated from pure domain/math/GPU cores so incremental builds and unit tests stay fast:
- `protocol`: `serde`-only wire types shared by daemon, client, persistence, and UI.
- `celestial`: Pure astronomy math, projections (`CameraProjection::for_fov`), and coordinate parsers (`parse_ra_degrees`, `parse_dec_degrees`).
- `mount_renderer`: Headless 3D mount kinematics, procedural meshes, attitude low-pass filter, and wgpu overlay renderer (no `gpui` or `bevy`/`planetarium`).
- `mount_viewer`: GPUI mount panels (`MountViewportPanel`, `MountControlPanel`), `MountViewerBridge`, and 2D gizmo painting.
- `settings`: Global app settings and shared `QuickLayer` / `LayerPillStyle` sky controls used by both `Explore` and `Mount`.
- `components`: Shared GPUI widgets (`Slider`, `TextField`, `StatusIndicator` + `*_status_presentation` helpers).
- `drivers/synscan` & `drivers/gphoto`: Hardware actors. Keep serial I/O behind `SerialTransport` so protocol/session logic stays unit-testable without hardware.

## Coordinate Conventions
- **Equatorial**: `celestial` / `planetarium` use ICRS J2000 (applying IAU 1976 precession in `ObservationContext::equatorial_to_topocentric_matrix`). SynScan serial telemetry is of-date (JNow).
- **Topocentric ENU** (`celestial`, `planetarium`): `+X = East`, `+Y = North`, `+Z = Up`. Azimuth `0 deg = North`, `90 deg = East`.
- **3D Mount World** (`mount_renderer`, `mount.wgsl`): Right-handed Y-up: `+X = East`, `+Y = Up`, `-Z = North`. Polar axis points along the elevated pole (`+Y >= 0` in both Northern and Southern hemispheres).

## Sharp Edges
- **GPUI `ElementId`**: Tuple IDs accept `(&str, usize)` / `(&str, u64)` / `(&str, EntityId)`, not `(&str, &str)`. Prefer tuple IDs over `format!()` inside `render()`.
- **GPUI `render()` size**: Break large panels into focused helper methods returning `AnyElement` to avoid deep generic builder monomorphization and slow compile times.
- **`wgpu` 29 `Send` recursion**: Deep `wgpu_core` types wrapped in closures can hit the default auto-trait recursion limit (`#![recursion_limit = "256"]` in `mount_viewer`). Avoid capturing `BevyBridge` inside frame-listener closures (it also creates an `Arc` cycle).
- **UI thread I/O**: Run disk persistence (`DocumentStore::save`) via `tokio::task::spawn_blocking`, never synchronously on the GPUI thread.

# Pipewire Soundpad (pwsp)

Rust soundboard for Linux that routes audio files through a PipeWire virtual microphone. Client/server architecture with three binaries sharing a Unix socket.

## Binaries

| Binary | Path | Role |
|--------|------|------|
| `pwsp-daemon` | `src/bin/daemon.rs` | Background server. Owns PipeWire virtual devices, audio playback, global hotkeys, Unix socket. |
| `pwsp-cli` | `src/bin/cli.rs` | Command-line client. Talks to daemon over Unix socket. |
| `pwsp-gui` | `src/main.rs` | egui/eframe GUI client. Also talks to daemon. |

The daemon must be running for clients to do anything. Systemd user unit at `assets/pwsp-daemon.service`.

## Commands

```bash
cargo build --release            # builds all 3 binaries
cargo run --bin pwsp-daemon      # run daemon (foreground, for dev)
cargo run --bin pwsp-cli -- ...  # CLI client
cargo run --bin pwsp-gui         # GUI client
cargo clippy --all-targets       # lint (CI uses this)
cargo fmt
```

Edition is **2024** — requires recent rustc. CI builds on Rust stable; clippy lints are enforced (see commit `a6d93ff`).

## Layout

```
src/
├── lib.rs             # re-exports types + utils
├── main.rs            # GUI entry
├── bin/{cli,daemon}.rs
├── types/             # data structures: audio_player, commands, config, gui, pipewire, socket
├── utils/             # impls: commands, config, daemon, global_hotkeys, gui, pipewire
└── gui/               # egui drawing + input + update loop
assets/                # icon, .desktop, systemd unit, screenshot
packages/{aur,flatpak,rpm}/  # distro packaging (each is its own thing)
```

`types/` defines structs/enums; `utils/` holds the logic. Socket protocol types live in `types/socket.rs` + `types/commands.rs` and are shared by daemon and clients.

## Key dependencies (pinned/notable)

- `pipewire = "0.9"` — the audio backend; do not abstract this away
- `rodio` — pinned to a **git rev**, not crates.io (`Cargo.toml:32-36`). If you bump it, also bump the rev.
- `egui`/`eframe` 0.34 with `glow` + both `x11` and `wayland` backends
- `evdev` w/ tokio feature — global hotkey capture (needs input device permissions)
- `tokio` full — async runtime everywhere

## Release profile quirks

`Cargo.toml:70-75` uses `opt-level = "z"`, `lto = true`, `codegen-units = 1`, `panic = "abort"`, `strip = true`. Release builds are slow and small. Don't switch to "3" without reason — these are intentional for distro packaging.

## Packaging

- `.deb` metadata is in `Cargo.toml` under `[package.metadata.deb]`
- AUR / Flatpak / RPM specs live in `packages/`
- Releases via `.github/workflows/release.yml`; Flatpak repo via `flatter.yml`

When changing binary names, install paths, or asset locations, update **all** of: `Cargo.toml [package.metadata.deb]`, `packages/aur/*/PKGBUILD`, `packages/flatpak/`, `packages/rpm/`, and `assets/pwsp-daemon.service`.

## Conventions

- Commit style: conventional-ish prefixes (`fix`, `refactor`, `fix(packages)`, etc.) — see `git log`.
- PRs target `main`; CI runs build + clippy.
- Version bumps touch `Cargo.toml` + `Cargo.lock` together (see `e6c8d72`).

## Gotchas

- **evdev needs permissions** — global hotkeys read `/dev/input/*`; user must be in `input` group or daemon needs the right caps. Failures here are not bugs in the code.
- **Two GUI backends** — eframe is built with both x11 and wayland; runtime selection is automatic. Don't drop one without checking distro packaging.
- **rodio is a git dep** — `cargo update -p rodio` won't work the usual way; edit the `rev` in `Cargo.toml`.
- **Rust 1.95 clippy** introduced new lints that were fixed in `a6d93ff`. If clippy explodes after a toolchain bump, that's the pattern to follow.

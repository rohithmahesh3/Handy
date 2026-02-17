# AGENTS.md

This file is the source-of-truth guidance for coding agents working in this repository.

## Scope

This project is a **Fedora Workstation + GNOME + Wayland only** speech-to-text app with native IBus integration.

- Main app: `handy`
- D-Bus daemon mode: `handy --daemon`
- IBus engine binary: `ibus-handy-engine`
- D-Bus service name: `com.handy.Transcription`

Do not design for non-GNOME desktops or non-Wayland input stacks unless explicitly asked.

## Build, Test, Run

### System dependencies (Fedora)

```bash
sudo dnf install -y \
    gtk4-devel \
    libadwaita-devel \
    graphene-devel \
    alsa-lib-devel \
    pipewire-devel \
    libevdev-devel \
    openssl-devel \
    ibus-devel \
    cmake \
    clang-devel \
    glslc
```

### Developer commands

```bash
# Build
cargo build
cargo build --release

# Lint / format / tests
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --all
cargo test

# Run UI app
cargo run --release

# Run daemon (D-Bus server)
cargo run --release -- --daemon

# Run IBus engine binary (testing)
cargo run --release --bin ibus-handy-engine --features cli -- --ibus
```

### Model setup (required for development)

```bash
mkdir -p resources/models
curl -o resources/models/silero_vad_v4.onnx https://blob.handy.computer/silero_vad_v4.onnx
```

### RPM build

```bash
./build-rpm.sh
```

`build-rpm.sh` generates these from templates before building:
- `packaging/fedora/ibus-handy.spec` from `packaging/fedora/ibus-handy.spec.in`
- `packaging/fedora/handy.xml` from `packaging/fedora/handy.xml.in`

## Runtime Architecture (Current)

### 1) Application processes

- `handy` without args: GTK4/Libadwaita preferences UI.
- `handy --daemon`: starts recording/model runtime and exports `com.handy.Transcription` on session bus.
- `ibus-handy-engine`: launched by IBus, handles IBus callbacks and global push-to-talk registration.

### 2) IBus engine stack

- C GObject wrapper in `ibus-sys/wrapper.c` defines `IBusHandyEngine`.
- Rust callback handlers are wired from `src/ibus_engine/context.rs`.
- `src/ibus_engine/ibus_api.rs` provides IBus global engine get/set helpers via FFI:
  - `ibus_handy_get_global_engine_name`
  - `ibus_handy_set_global_engine`

### 3) D-Bus interface

Bus/object/interface:
- Bus: `com.handy.Transcription`
- Path: `/com/handy/Transcription`
- Interface: `com.handy.Transcription`

Methods:
- `StartRecording()`
- `StopRecording() -> string`
- `CancelRecording()`
- `GetState() -> (bool is_recording, bool has_model_selected)`
- `GetLanguage() -> string`
- `SetLanguage(string)`

Signals:
- `TranscriptionReady(string)`
- `RecordingStateChanged(bool)`
- `Error(string)`

## Recording Modes

Configured by GSettings key `recording-mode`:

- `auto`:
  - Enter/focus Handy source starts recording.
  - Leaving/disabling source stops recording and commits final text.

- `push_to_talk`:
  - Local PTT works through key events in `context.rs` when Handy source is active.
  - Global PTT is implemented in `global_shortcuts.rs` through XDG Desktop Portal `GlobalShortcuts`.

## Global Push-to-Talk (Portal)

File: `src/ibus_engine/global_shortcuts.rs`

Behavior:
- Watches these settings live:
  - `recording-mode`
  - `push-to-talk-keyval`
  - `push-to-talk-modifiers`
- Creates portal session (`CreateSession`), binds shortcut (`BindShortcuts`), reads active shortcut (`ListShortcuts`), and listens to:
  - `Activated`
  - `Deactivated`
  - `ShortcutsChanged`
- On `Activated`:
  - Start recording over D-Bus
  - Save previous engine
  - Switch to Handy engine
- On `Deactivated`:
  - Restore previous engine

Important:
- Shortcut re-registration is automatic on settings changes (no engine restart needed).
- Portal request lifecycle uses request-handle token paths and explicit session close (`org.freedesktop.portal.Session.Close`).

## Settings and Live Sync

Schema: `data/com.handy.Transcription.gschema.xml`  
Rust wrapper: `src/settings.rs`

Notable recording keys:
- `recording-mode` (`auto` / `push_to_talk`)
- `push-to-talk-keyval` (`u32` keyval)
- `push-to-talk-modifiers` (`u32` bitmask)
- `mute-while-recording`

UI capture for PTT shortcut:
- `src/ui/pages/general.rs`
- Uses `EventControllerKey` and stores keyval/modifiers into GSettings.

Daemon runtime applies many settings live in `src/app.rs` via `connect_changed(...)`.

## Critical Implementation Constraints

1. **Do not reintroduce shell-based `ibus engine` switching.**  
Use `src/ibus_engine/ibus_api.rs` (FFI-backed global engine get/set).

2. **Do not block IBus callback threads with long operations.**  
`StopRecording`/transcription path is offloaded and commits back on GLib main context.

3. **When touching engine lifecycle, preserve GObject safety pattern.**  
`context.rs` refs engine object before async work and unrefs after commit.

4. **Keep portal/global shortcut code resilient.**  
Handle response codes and session cleanup; avoid leaking portal sessions.

## Important Files

Core:
- `src/main.rs` - app entry, chooses UI vs daemon mode
- `src/app.rs` - UI startup and daemon runtime wiring
- `src/dbus/server.rs` - `com.handy.Transcription` server
- `src/settings.rs` - typed GSettings wrapper
- `data/com.handy.Transcription.gschema.xml` - schema

IBus:
- `src/bin/ibus-handy-engine.rs` - engine binary entrypoint
- `src/ibus_engine/context.rs` - IBus callbacks and commit flow
- `src/ibus_engine/global_shortcuts.rs` - global PTT via portal
- `src/ibus_engine/ibus_api.rs` - IBus global engine helper API
- `ibus-sys/wrapper.c` / `ibus-sys/wrapper.h` - C wrapper and helper exports
- `ibus-sys/src/lib.rs` - Rust FFI declarations

UI:
- `src/ui/window.rs`
- `src/ui/pages/general.rs`
- `src/ui/pages/models.rs`
- `src/ui/pages/advanced.rs`

Packaging:
- `build-rpm.sh`
- `packaging/fedora/ibus-handy.spec.in`
- `packaging/fedora/handy.xml.in`
- `packaging/fedora/handy.service`
- `packaging/fedora/com.handy.Transcription.service`

## Coding Conventions

- Rust edition: 2021
- Run before finalizing:
  - `cargo fmt --all`
  - `cargo clippy --all-targets --all-features -- -D warnings`
  - `cargo test`
- Keep logic in Rust where possible; C wrapper should stay thin.
- Prefer adding behavior through existing settings + runtime sync instead of one-off env flags.

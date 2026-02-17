# AGENTS.md

Source-of-truth instructions for coding agents in this repository.

## Project Scope

Handy is a **Fedora Workstation + GNOME + Wayland** speech-to-text project with IBus integration.

Targets in scope:
- `handy` (GTK4/libadwaita preferences UI)
- `handy --daemon` (recording/transcription D-Bus runtime)
- `ibus-handy-engine` (IBus engine process)

Out of scope unless explicitly requested:
- Non-GNOME desktop support
- Non-Wayland input stack support
- Generic cross-distro abstractions that weaken Fedora/GNOME behavior

## Build, Test, Run

### Fedora dependencies

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

# Quality gates
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo test

# Run UI
cargo run --release

# Run daemon
cargo run --release -- --daemon

# Run IBus engine (dev/testing)
cargo run --release --bin ibus-handy-engine --features cli -- --ibus
```

### Model bootstrap for development

```bash
mkdir -p resources/models
curl -o resources/models/silero_vad_v4.onnx https://blob.handy.computer/silero_vad_v4.onnx
```

## Packaging Workflow

RPM build entrypoint:

```bash
./build-rpm.sh
```

`build-rpm.sh` generates these files from templates at build time:
- `packaging/fedora/ibus-handy.spec` from `packaging/fedora/ibus-handy.spec.in`
- `packaging/fedora/handy.xml` from `packaging/fedora/handy.xml.in`

Generated files are not intended to be committed.

## Runtime Architecture

### Process model

- `handy`: preferences UI only.
- `handy --daemon`: owns recording state, transcription, D-Bus API, global PTT registration.
- `ibus-handy-engine`: IBus callbacks and commit path to focused app.

### D-Bus contract

Service:
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

## Push-to-Talk Behavior

Handy is push-to-talk only.

Settings keys used for PTT:
- `push-to-talk-keyval` (GDK keyval stored in GSettings)
- `push-to-talk-modifiers` (GDK modifier bitmask)

Global PTT uses **evdev** for keyboard monitoring (`src/global_shortcuts.rs`):
1. On startup: discover keyboard devices in `/dev/input/event*`, open event streams.
2. GDK keyvals from settings are mapped to evdev keycodes via `src/key_mapping.rs`.
3. On press: store current engine, switch to Handy engine, call `StartRecording`.
4. On release: restore previous engine.
5. Engine disable callback handles stop/transcribe/commit path.
6. Release watchdog checks for stuck recording and calls `CancelRecording` as fallback.

This approach requires read access to `/dev/input/event*` devices. A udev rule
(`packaging/fedora/90-handy-input.rules`) ensures `uaccess` for the active desktop user.

## Critical Constraints

1. Do not reintroduce shell-based input-source switching.
Use FFI-backed helpers (`src/ibus_control.rs`, `src/ibus_engine/ibus_api.rs`).

2. Do not block IBus callback threads with long operations.
Stop/transcribe work must remain off callback thread.

3. Preserve GObject lifetime safety in async commit paths.
Keep ref/unref pattern intact in `src/ibus_engine/context.rs`.

4. Keep evdev device lifecycle clean.
   Close device streams on session restart; abort reader tasks on config change.

5. Keep daemon state transitions consistent.
`RecordingStateChanged(false)` should happen immediately when stop starts, not after long transcription.

## Settings and Feature Notes

Schema file: `data/com.handy.Transcription.gschema.xml`.

Current active behavior:
- Push-to-talk recording
- Optional audio feedback sounds
- Optional LLM post-processing on final transcript

Removed/obsolete paths should not be reintroduced without product decision:
- `recording-mode` auto mode
- realtime partial transcription settings

## Key Source Files

Core:
- `src/main.rs`
- `src/app.rs`
- `src/dbus/server.rs`
- `src/settings.rs`

IBus and PTT:
- `src/global_shortcuts.rs`
- `src/key_mapping.rs`
- `src/ibus_engine/context.rs`
- `src/ibus_control.rs`
- `src/ibus_engine/ibus_api.rs`
- `src/bin/ibus-handy-engine.rs`
- `ibus-sys/wrapper.c`
- `ibus-sys/wrapper.h`

Models/audio:
- `src/managers/model.rs`
- `src/managers/audio.rs`
- `src/managers/transcription.rs`
- `src/audio_toolkit/audio/recorder.rs`
- `src/audio_feedback.rs`

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
- `packaging/fedora/90-handy-input.rules`

## Definition of Done for Agent Changes

Before finalizing any non-trivial change:
1. Run `cargo fmt --all`.
2. Run `cargo clippy --all-targets --all-features -- -D warnings`.
3. Run `cargo test`.
4. If packaging/schema changed, ensure generated artifacts are handled correctly and docs remain aligned.

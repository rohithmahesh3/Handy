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
- `handy --daemon`: owns recording state, transcription, D-Bus API, global PTT registration, evdev keyboard monitoring.
- `ibus-handy-engine`: IBus callbacks and commit path to focused app.

### D-Bus contract

Service:
- Bus: `com.handy.Transcription`
- Path: `/com/handy/Transcription`
- Interface: `com.handy.Transcription`

Methods:
- `StartRecording()`
- `StopRecording() -> string`
- `StartRecordingSession() -> u64`
- `StopRecordingSession(u64) -> string`
- `CancelRecording()`
- `GetState() -> (bool is_recording, bool has_model_selected)`
- `GetLatestPartial() -> (u64 session_id, u64 sequence_id, string text)`
- `GetPttDiagnostics() -> (bool, string, string, string, u64, bool, bool, u64, u64, u64)`
- `GetLanguage() -> string`
- `SetLanguage(string)`

Signals:
- `TranscriptionReady(string)`
- `RecordingStateChanged(bool)`
- `PartialTranscriptionReady(u64 session_id, u64 sequence_id, string text)`
- `Error(string)`

### Transcription cache

`HandyState` keeps a `last_transcription_cache`. When `StopRecording` produces text,
the result is cached. If a second `StopRecording` arrives and recording has already
stopped (e.g. from the engine process's `disable()` → `stop_and_commit()` path), the
cached text is returned and cleared. This enables the deferred stop-and-restore PTT flow.

## Push-to-Talk Behavior

Handy is push-to-talk only.

Settings keys used for PTT:
- `push-to-talk-keyval` (GDK keyval stored in GSettings)
- `push-to-talk-modifiers` (GDK modifier bitmask)

Global PTT uses **evdev** for keyboard monitoring (`src/global_shortcuts.rs`):
1. On startup: discover keyboard devices in `/dev/input/event*`, open event streams.
2. GDK keyvals from settings are mapped to evdev keycodes via `src/key_mapping.rs`.
3. On press: store current engine, switch to Handy engine, call `StartRecording`.
4. On release (**deferred stop-and-restore**):
   a. `StopRecording` is called via D-Bus first — blocks until transcription completes.
   b. Only after `StopRecording` returns does the daemon restore the previous engine.
   c. Restoring the engine triggers `context.rs:disable()` in the engine process.
   d. `disable()` calls `stop_and_commit()` which retrieves the cached transcription
      and commits it via `ibus_engine_commit_text` while the engine pointer is still valid.
   e. After commit, `switch_engine_async` restores the previous input source.
5. Release watchdog checks for stuck recording and calls `CancelRecording` as fallback.

This ordering is critical: `ibus_engine_commit_text` does not reliably deliver text
after the engine has been disabled. The Handy engine must remain active during the
full transcription + commit cycle.

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

6. Never restore the IBus engine before `StopRecording` completes.
   The deferred stop-and-restore pattern in `global_shortcuts.rs` prevents commit-after-disable.
   `context.rs:disable()` must pass `last_non_handy_engine` to `stop_and_commit()`.

## Settings and Feature Notes

Schema file: `data/com.handy.Transcription.gschema.xml`.

Current active behavior:
- Push-to-talk recording
- Optional audio feedback sounds
- Optional LLM post-processing on final transcript
- Live partial transcription (configurable interval, minimum delta, max history)

Removed/obsolete paths should not be reintroduced without product decision:
- `recording-mode` auto mode

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

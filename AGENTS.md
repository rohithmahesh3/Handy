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
- `StartRecordingSessionForTarget(u64 target_engine_id) -> u64`
- `StopRecordingSession(u64) -> string`
- `CancelRecording()`
- `GetState() -> (bool is_recording, bool has_model_selected)`
- `GetPttDiagnostics() -> (bool, string, string, string, u64, bool, bool, u64, u64, u64)`
- `GetPttDiagnosticsVerbose() -> string` (JSON)
- `GetPttRecentEvents() -> array<string>`
- `TakePendingCommitForEngine(u64 engine_id) -> (u64, string)`
- `GetPendingCommitStats() -> string` (JSON)
- `SetFocusedEngine(u64 engine_id, bool focused)`
- `GetFocusedEngine() -> (u64 focused_engine_id, u64 last_change_ms)`
- `GetRecentLogs() -> array<string>`
- `GetLanguage() -> string`
- `SetLanguage(string)`

Signals:
- `TranscriptionReady(string)`
- `RecordingStateChanged(bool)`
- `Error(string)`

### Pending commit handoff

`HandyState` stores final transcripts in:
- `last_transcription_cache` (compatibility fallback)
- `pending_commit` (bounded queue handoff consumed by engine via `TakePendingCommitForEngine`)

Important behavior:
- Start recording does **not** clear pending commit.
- `CancelRecording` does **not** clear pending commit.
- `pending_commit` stores `(session_id, target_engine_id, text)` and keeps up to 32 items, dropping oldest when full.
- Debug transcription testing does **not** drain pending commits.
- PTT does **not** block on pending queue drain before starting a new session.

## Shortcut Behavior

Handy uses a global press-to-toggle dictation shortcut.

Settings keys used for the global shortcut:
- `dictation-shortcut-keyval` (GDK keyval stored in GSettings)
- `dictation-shortcut-modifiers` (GDK modifier bitmask)

Global PTT uses **evdev** (`src/global_shortcuts.rs`):
1. Discover keyboard devices in `/dev/input/event*`, open event streams.
2. Resolve GDK keyval+modifiers to evdev keycodes (`src/key_mapping.rs`).
3. On press while idle:
   - switch to Handy engine (verified),
   - verify focused-context activation via daemon `GetFocusedEngine`,
   - call `StartRecordingSessionForTarget(focused_engine_id)`.
4. On next press while recording:
   - call `StopRecordingSession` and wait for result,
   - do **not** auto-restore input source in PTT path.
5. Final text delivery:
   - engine-side pending commit listener (`src/ibus_engine/context.rs`) polls `TakePendingCommitForEngine(engine_id)`,
   - commits via `ibus_engine_commit_text` on GTK main context while engine is active.
6. `disable()` still performs a final `TakePendingCommitForEngine(engine_id)` consume as fallback.

This architecture intentionally avoids autoswitch restore races.

This approach requires read access to `/dev/input/event*` devices. A udev rule
(`packaging/fedora/90-handy-input.rules`) ensures `uaccess` for the active desktop user.

## Critical Constraints

1. Do not reintroduce shell-based input-source switching.
   Use FFI-backed helpers (`src/ibus_control.rs`, `ibus-sys/wrapper.c`).

2. Do not block IBus callback threads with long operations.
   Stop/transcribe work stays in daemon; engine commit listener runs in worker thread and invokes GTK main context for UI/IBus operations.

3. Preserve GObject lifetime safety in async commit paths.
   Keep ref/unref pattern intact in `src/ibus_engine/context.rs`.

4. Keep evdev device lifecycle clean.
   Close device streams on session restart; abort reader tasks on config change.

5. Keep daemon state transitions consistent.
   `RecordingStateChanged(false)` should happen immediately when stop starts, not after long transcription.

6. Keep commit delivery single-path in PTT flow.
   Do not reintroduce direct restore/commit in `global_shortcuts.rs`.

## Settings and Feature Notes

Schema file: `data/com.handy.Transcription.gschema.xml`.

Current active behavior:
- Push-to-talk recording
- Optional audio feedback sounds
- Optional LLM post-processing on final transcript

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
- `src/ui/pages/debug.rs`

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

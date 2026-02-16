# AGENTS.md

This file provides guidance to AI coding agents working in this repository.

## Overview

This is a **Fedora/GNOME-only fork** of Handy with native IBus input method integration. It targets Fedora Workstation on Wayland only.

## Build/Lint/Test Commands

**Prerequisites:**

- [Rust](https://rustup.rs/) (latest stable)
- GTK4, Libadwaita, GStreamer, IBus development packages

```bash
# Install dependencies (Fedora)
sudo dnf install -y \
    gtk4-devel \
    libadwaita-devel \
    gstreamer1-devel \
    gstreamer1-plugins-base-devel \
    graphene-devel \
    alsa-lib-devel \
    pipewire-devel \
    libevdev-devel \
    openssl-devel \
    ibus-devel \
    cmake \
    clang-devel \
    glslc

# Development build
cargo build

# Production build
cargo build --release

# Linting
cargo clippy --all-targets --all-features -- -D warnings

# Formatting
cargo fmt

# Testing
cargo test
cargo test <test_name>

# Run main application
cargo run --release

# Run IBus engine (for testing)
cargo run --release --bin ibus-handy-engine -- --ibus
```

**Model Setup (Required for Development):**

```bash
mkdir -p resources/models
curl -o resources/models/silero_vad_v4.onnx https://blob.handy.computer/silero_vad_v4.onnx
```

**RPM Build:**

```bash
./build-rpm.sh
```

## Architecture Overview

Handy is a speech-to-text application for Fedora Workstation/GNOME/Wayland with native IBus integration.

### Core Components

**Application (Rust - src/):**

- `main.rs` - Entry point for main GTK application
- `app.rs` - GTK application setup and initialization
- `lib.rs` - Module exports
- `managers/` - Core business logic (audio.rs, model.rs, transcription.rs, history.rs)
- `audio_toolkit/` - Low-level audio processing (device, recorder, resampler, VAD)
- `settings.rs` - Application settings with GSettings (dconf)
- `shortcut/` - Stub (IBus handles shortcuts)
- `dbus/` - D-Bus server for IBus communication
- `ibus_engine/` - Rust IBus engine implementation
  - `context.rs` - Engine state and D-Bus client
  - `mod.rs` - Module exports
- `ui/` - GTK4/Libadwaita UI components
  - `window.rs` - Main preferences window
  - `sidebar.rs` - Navigation sidebar
  - `pages/` - Settings pages (general, models, advanced, history, etc.)
  - `widgets/` - Reusable UI widgets

**IBus Engine Binary (Rust - src/bin/):**

- `ibus-handy-engine.rs` - Separate binary launched by IBus daemon
  - Uses C wrapper (wrapper.c) for GObject integration
  - Communicates with main app via D-Bus

**IBus Bindings (Rust - ibus-sys/):**

- `src/lib.rs` - FFI bindings to libibus-1.0
- `wrapper.c/h` - C glue code for GObject subclassing
- `build.rs` - Compiles C wrapper and links IBus

**Packaging (packaging/fedora/):**

- `ibus-handy.spec` - Fedora RPM spec file
- `handy.desktop` - Desktop entry file
- `handy.service` - Systemd user service for autostart
- `handy.xml` - IBus component registration
- `com.handy.Transcription.service` - D-Bus service activation

**Data (data/):**

- `com.handy.Transcription.gschema.xml` - GSettings schema

**Resources (resources/):**

- `icons/handy.svg` - Application icon
- `models/` - ML models (VAD, etc.)
- `*.wav` - Audio feedback sounds

### Key Patterns

- **Manager Pattern:** Core functionality in managers (Audio, Model, Transcription, History)
- **GSettings:** Persistent settings via dconf/GSettings
- **D-Bus Server:** Always-on D-Bus server (`com.handy.Transcription`) for IBus communication
- **Hybrid IBus Engine:** C wrapper for GObject + Rust callbacks
- **Pipeline Processing:** Audio → VAD → Whisper → Text output

### IBus Engine Architecture

```
┌─────────────────────────────────────────────┐
│  IBus Daemon (C)                             │
└──────────────────┬──────────────────────────┘
                   │ GObject signals
┌──────────────────▼──────────────────────────┐
│  wrapper.c (C)                               │
│  - Defines IBusHandyEngine GObject class     │
│  - Implements IBusEngineClass vfuncs         │
│  - Bridges to Rust via function pointers     │
└──────────────────┬──────────────────────────┘
                   │ C function pointers
┌──────────────────▼──────────────────────────┐
│  Rust (src/ibus_engine/)                     │
│  - HandyContext holds engine state           │
│  - Callbacks handle focus/key events         │
│  - D-Bus client talks to main Handy app      │
└──────────────────────────────────────────────┘
```

### D-Bus Interface

**Bus Name:** `com.handy.Transcription`
**Object Path:** `/com/handy/Transcription`

**Methods:**

- `StartRecording()` - Start recording audio
- `StopRecording()` → `string` - Stop and return transcribed text
- `CancelRecording()` - Cancel without transcribing
- `GetState()` → `(bool, bool)` - Get (is_recording, is_model_loaded)
- `GetLanguage()` → `string` - Get current language
- `SetLanguage(string)` - Set language for transcription

**Signals:**

- `TranscriptionReady(string)` - Emitted when transcription is complete
- `RecordingStateChanged(bool)` - Emitted when recording state changes
- `Error(string)` - Emitted on errors

## Code Style Guidelines

### Rust

**Imports:**

```rust
// std first, then external crates, then local modules
use std::sync::{Arc, Mutex};
use log::{debug, error, info};
use gtk::prelude::*;
use crate::settings::{get_settings, AppSettings};
```

**Naming:**

- Types: PascalCase (`AudioRecordingManager`, `RecordingState`)
- Functions: snake_case (`try_start_recording`, `get_effective_microphone_device`)
- Modules: snake_case (`audio_toolkit`, `llm_client`)
- Constants: SCREAMING_SNAKE_CASE (`WHISPER_SAMPLE_RATE`)

**Error Handling:**

- Use `anyhow::Error` for application errors
- Use `map_err(|e| format!("Failed to...: {}", e))?` for error context

**Formatting:**

- `cargo fmt` (Rust 2021 edition)
- Section comments: `/* ----- section name ----- */`

### C (IBus Wrapper)

- Follow GObject conventions for class structure
- Use `ibus_` prefix for IBus-related functions
- Keep wrapper minimal - logic goes in Rust

## Technology Stack

**Core Libraries:**

- `transcribe-rs` - Local speech recognition (Whisper, Parakeet, Moonshine, SenseVoice)
- `cpal` - Audio I/O (ALSA/PipeWire)
- `vad-rs` - Voice Activity Detection (Silero)
- `zbus` - D-Bus communication

**UI:**

- GTK4 for widget toolkit
- Libadwaita for GNOME-styled components
- GSettings/dconf for persistent configuration

**Backend:**

- Tokio for async runtime
- rusqlite for local database (history)

**IBus:**

- `ibus-sys` - Rust FFI bindings to libibus-1.0
- C wrapper for GObject subclassing
- Direct D-Bus calls to Handy

## Platform Support

This fork supports **Fedora Workstation on GNOME/Wayland only**.

- Requires IBus >= 1.5.0
- Requires PipeWire for audio
- Uses GSettings for configuration

## Important Files

- `src/lib.rs` - Module exports
- `src/app.rs` - GTK application initialization
- `src/dbus/server.rs` - D-Bus server implementation
- `src/ibus_engine/context.rs` - IBus engine implementation
- `src/settings.rs` - Settings schema and GSettings integration
- `src/ui/window.rs` - Main preferences window
- `ibus-sys/wrapper.c` - C wrapper for IBus GObject
- `data/com.handy.Transcription.gschema.xml` - GSettings schema
- `packaging/fedora/ibus-handy.spec` - Fedora RPM specification
- `packaging/fedora/handy.xml` - IBus component registration

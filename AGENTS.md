# AGENTS.md

This file provides guidance to AI coding agents working in this repository.

## Build/Lint/Test Commands

**Prerequisites:**

- [Rust](https://rustup.rs/) (latest stable)
- [Bun](https://bun.sh/) package manager

```bash
# Install dependencies
bun install

# Development
bun run tauri dev              # Full app in dev mode
bun run dev                    # Frontend only (Vite dev server)

# Production build
bun run tauri build            # Build full application
bun run build                  # Build frontend only (TypeScript + Vite)

# Linting
bun run lint                   # ESLint on src/
bun run lint:fix               # ESLint with auto-fix
cargo clippy --all-targets --all-features -- -D warnings  # Rust linting

# Formatting
bun run format                 # Prettier + cargo fmt
bun run format:check           # Check formatting without changes
bun run format:frontend        # Prettier only
bun run format:backend         # cargo fmt only

# Testing
bun run test:playwright        # Run all Playwright tests
bun run test:playwright:ui     # Playwright tests with UI
cargo test                     # Run all Rust tests
cargo test <test_name>         # Run single Rust test

# Utilities
bun run check:translations     # Check translation completeness
```

**Model Setup (Required for Development):**

```bash
mkdir -p src-tauri/resources/models
curl -o src-tauri/resources/models/silero_vad_v4.onnx https://blob.handy.computer/silero_vad_v4.onnx
```

## Architecture Overview

Handy is a cross-platform desktop speech-to-text application built with Tauri (Rust backend + React/TypeScript frontend).

### Core Components

**Backend (Rust - src-tauri/src/):**

- `lib.rs` - Main entry point with Tauri setup, tray menu, managers
- `managers/` - Core business logic (audio.rs, model.rs, transcription.rs, history.rs)
- `audio_toolkit/` - Low-level audio processing (device, recorder, resampler, VAD)
- `commands/` - Tauri command handlers (audio.rs, models.rs, transcription.rs, history.rs)
- `settings.rs` - Application settings with Tauri store
- `shortcut/` - Global keyboard shortcut handling

**Frontend (React/TypeScript - src/):**

- `App.tsx` - Main component with onboarding flow
- `components/` - UI components (settings/, ui/, model-selector/, onboarding/)
- `stores/` - Zustand state management (settingsStore.ts, modelStore.ts)
- `hooks/` - React hooks for settings and model management
- `bindings.ts` - Auto-generated types from tauri-specta (DO NOT EDIT)
- `i18n/` - Internationalization with i18next

### Key Patterns

- **Manager Pattern:** Core functionality in managers (Audio, Model, Transcription, History) initialized at startup
- **Command-Event Architecture:** Frontend calls Tauri commands, backend emits events for updates
- **Pipeline Processing:** Audio → VAD → Whisper → Text output

## Code Style Guidelines

### TypeScript/React

**Imports:**

```typescript
// External imports first, then internal
import { create } from "zustand";
import type { AppSettings } from "@/bindings";
import { commands } from "@/bindings";
```

**Types:**

- Use `type` for type-only imports: `import type { Foo } from "./types"`
- Prefer interfaces for object shapes, type for unions/primitives
- All types from backend are in `bindings.ts` (auto-generated via `tauri-specta`)

**Naming:**

- Components: PascalCase (`Button.tsx`, `ModelSelector.tsx`)
- Hooks: camelCase with `use` prefix (`useSettings.ts`)
- Stores: camelCase with `Store` suffix (`settingsStore.ts`)
- Functions: camelCase (`refreshSettings`, `updateSetting`)

**State Management:**

- Zustand with `subscribeWithSelector` middleware
- Async actions with try/catch and console.error for failures
- Optimistic updates with rollback on error

**JSX:**

- All user-facing text must use i18next: `{t("key")}`
- ESLint rule `i18next/no-literal-string` enforces this

**Formatting:**

- Prettier with LF line endings (`.prettierrc`)
- Tailwind CSS for styling

### Rust

**Imports:**

```rust
// std first, then external crates, then local modules
use std::sync::{Arc, Mutex};
use log::{debug, error, info};
use tauri::{AppHandle, Manager};
use crate::settings::{get_settings, AppSettings};
```

**Naming:**

- Types: PascalCase (`AudioRecordingManager`, `RecordingState`)
- Functions: snake_case (`try_start_recording`, `get_effective_microphone_device`)
- Modules: snake_case (`audio_toolkit`, `llm_client`)
- Constants: SCREAMING_SNAKE_CASE (`WHISPER_SAMPLE_RATE`)

**Error Handling:**

- Use `anyhow::Error` for application errors
- Tauri commands return `Result<T, String>` with descriptive error messages
- Use `map_err(|e| format!("Failed to...: {}", e))?` for error context

**Commands:**

```rust
#[tauri::command]
#[specta::specta]
pub fn my_command(app: AppHandle, arg: String) -> Result<MyType, String> {
    // Implementation
}
```

**Specta:**

- All command types must derive `Serialize` and `specta::Type`
- Run type regeneration after adding/modifying commands (build.rs handles this)

**Formatting:**

- `cargo fmt` (Rust 2021 edition)
- Section comments: `/* ----- section name ----- */`

## Technology Stack

**Core Libraries:**

- `whisper-rs` / `transcribe-rs` - Local speech recognition
- `cpal` - Cross-platform audio I/O
- `vad-rs` - Voice Activity Detection (Silero)
- `tauri-specta` - Type-safe IPC with auto-generated TypeScript bindings

**Frontend:**

- React 18 + TypeScript 5.6 (strict mode)
- Zustand for state management
- Tailwind CSS 4 for styling
- i18next for internationalization
- React-Select for dropdowns

**Backend:**

- Tauri 2.x for desktop framework
- Tokio for async runtime
- rusqlite for local database (history)

## Platform-Specific Notes

- **macOS:** Metal acceleration, accessibility permissions, clamshell detection
- **Windows:** Vulkan acceleration, code signing, COM for audio mute
- **Linux:** GTK layer shell, PipeWire/PulseAudio/ALSA for audio

## Important Files

- `src/bindings.ts` - Auto-generated, DO NOT EDIT
- `src-tauri/src/lib.rs` - Main app initialization
- `src/stores/settingsStore.ts` - Central settings state
- `src-tauri/src/settings.rs` - Settings schema and defaults

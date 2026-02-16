# Handy - Speech to Text for Fedora/GNOME

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)]()

Handy is a speech-to-text application for **Fedora Workstation on GNOME/Wayland**. It integrates natively with IBus, allowing you to dictate text into any application.

This is a fork of [Handy](https://github.com/cjpais/Handy) specifically optimized for Fedora Workstation with native IBus integration.

## Features

- **Native IBus Integration** - Works like any other input method (Super+Space)
- **Local Processing** - Uses Whisper for offline speech recognition
- **Multi-language Support** - Supports 50+ languages
- **Visual Feedback** - System tray indicator and optional overlay
- **Transcription History** - Review and manage past transcriptions

## Installation

```bash
sudo dnf install handy
```

After installation, Handy automatically registers with IBus.

## Setup

1. Open **Settings → Keyboard → Input Sources**
2. Click the **+** button
3. Select **Handy Speech-to-Text**
4. Click **Add**

## Usage

| Action                   | Result                |
| ------------------------ | --------------------- |
| `Super+Space` to Handy   | Start recording       |
| Speak                    | Audio is captured     |
| `Super+Space` to next IM | Text committed to app |

### Preferences

Open Handy from the application menu or click **Preferences** in Keyboard Settings → Input Sources → Handy.

## Requirements

- Fedora 40+ Workstation
- GNOME on Wayland
- IBus (default on Fedora Workstation)
- Microphone

## Building from Source

```bash
# Install build dependencies
sudo dnf install rustc cargo python3-devel meson \
    gtk3-devel gtk-layer-shell-devel webkit2gtk4.1-devel \
    alsa-lib-devel pipewire-devel openssl-devel

# Clone and build
git clone -b fedora-gnome https://github.com/rohithmahesh/Handy.git
cd Handy

# Build the Rust backend
cd src-tauri
cargo build --release

# Build the IBus engine
cd ../ibus
meson setup builddir --prefix=/usr
cd builddir
meson compile

# Install (requires sudo)
sudo meson install
```

## How It Works

1. **D-Bus Server**: Handy starts a D-Bus server (`com.handy.Transcription`) on launch
2. **IBus Engine**: The `handy-ibus` engine connects to Handy via D-Bus
3. **Recording**: When you switch to Handy IM, the engine signals Handy to start recording
4. **Transcription**: When you switch away, Handy transcribes and sends text back to IBus
5. **Commit**: IBus commits the text to the focused application

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    Fedora Workstation                       │
├─────────────────────────────────────────────────────────────┤
│  User presses Super+Space → IBus switches to Handy IM      │
│                               │                             │
│                               ▼                             │
│  ┌──────────────┐    D-Bus   ┌──────────────┐              │
│  │ handy-ibus   │◄──────────►│    Handy     │              │
│  │ (IBus Engine)│             │ (Background) │              │
│  └──────────────┘             └──────────────┘              │
│         │                                                    │
│         │ IBus commit                                        │
│         ▼                                                    │
│  ┌──────────────┐                                           │
│  │ Focused App  │                                           │
│  └──────────────┘                                           │
└─────────────────────────────────────────────────────────────┘
```

## Model Support

Handy supports multiple speech recognition models:

- **Whisper Models** (Small/Medium/Turbo/Large) with GPU acceleration
- **Parakeet V3** - CPU-optimized model with automatic language detection

Models are downloaded automatically when you first use Handy.

## Troubleshooting

### Handy not appearing in IBus

```bash
# Refresh IBus cache
ibus write-cache
ibus restart
```

### No microphone access

Ensure your user is in the `audio` group:

```bash
sudo usermod -aG audio $USER
# Log out and back in
```

### Model download fails

Check your network connection. If behind a proxy, see [Manual Model Installation](#manual-model-installation) below.

### Manual Model Installation

If you can't download models automatically:

1. Find your app data directory: `~/.config/com.handy.handy/`
2. Create a `models` folder
3. Download models from:
   - Whisper Small: `https://blob.handy.computer/ggml-small.bin`
   - Parakeet V3: `https://blob.handy.computer/parakeet-v3-int8.tar.gz`
4. Place `.bin` files directly in `models/`
5. Extract `.tar.gz` files to `models/parakeet-tdt-0.6b-v3-int8/`

## Related Projects

- **[Handy (upstream)](https://github.com/cjpais/Handy)** - The original cross-platform version
- **[handy.computer](https://handy.computer)** - Project website

## License

MIT License - see [LICENSE](LICENSE) file for details.

## Acknowledgments

- **Handy** by cjpais - The original application
- **Whisper** by OpenAI - Speech recognition model
- **IBus** - Intelligent Input Bus

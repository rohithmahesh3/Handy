# Handy - Speech to Text for Fedora/GNOME

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)]()

Handy is a speech-to-text application for **Fedora Workstation on GNOME/Wayland**. It integrates natively with IBus, allowing you to dictate text into any application.

This is a fork of [Handy](https://github.com/cjpais/Handy) specifically optimized for Fedora Workstation with native IBus integration.

## Features

- **Native IBus Integration** - Works like any other input method (Super+Space)
- **Local Processing** - Uses Whisper/Parakeet for offline speech recognition
- **Multi-language Support** - Supports 50+ languages
- **Native GTK4/Libadwaita UI** - GNOME-native preferences window
- **Post-Processing** - Optional AI cleanup of transcripts via LLM

## Installation

```bash
sudo dnf install ibus-handy
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

Open Handy from the application menu to configure:

- Language selection
- Audio feedback
- Model management
- Post-processing settings

## Requirements

- Fedora 40+ Workstation
- GNOME on Wayland
- IBus (default on Fedora Workstation)
- Microphone

## Building from Source

```bash
# Install build dependencies
sudo dnf install -y \
    gtk4-devel libadwaita-devel graphene-devel \
    alsa-lib-devel pipewire-devel libevdev-devel \
    openssl-devel ibus-devel cmake clang-devel glslc

# Clone and build
git clone -b fedora-gnome https://github.com/rohithmahesh/Handy.git
cd Handy
cargo build --release
```

## How It Works

1. **D-Bus Server**: Handy starts a D-Bus server (`com.handy.Transcription`) on launch
2. **IBus Engine**: The `ibus-handy-engine` connects to Handy via D-Bus
3. **Recording**: When you switch to Handy IM, the engine signals Handy to start recording
4. **Transcription**: When you switch away, Handy transcribes and sends text back to IBus
5. **Commit**: IBus commits the text to the focused application

## Model Support

Handy supports multiple speech recognition models:

- **Whisper** (Small/Medium/Turbo) - OpenAI's speech recognition
- **Parakeet V3** - CPU-optimized with automatic language detection
- **SenseVoice** - Fast Chinese/English/Japanese/Korean

Models are downloaded from the preferences window.

## Troubleshooting

### Handy not appearing in IBus

```bash
ibus write-cache
ibus restart
```

### No microphone access

Ensure your user is in the `audio` group:

```bash
sudo usermod -aG audio $USER
# Log out and back in
```

### Manual Model Installation

Place models in `~/.local/share/handy/models/`:

- Whisper: `.bin` files directly
- Parakeet/SenseVoice: extract `.tar.gz` to subdirectory

## Related Projects

- **[Handy (upstream)](https://github.com/cjpais/Handy)** - The original cross-platform version
- **[handy.computer](https://handy.computer)** - Project website

## License

MIT License - see [LICENSE](LICENSE) file for details.

## Acknowledgments

- **Handy** by cjpais - The original application
- **Whisper** by OpenAI - Speech recognition model
- **IBus** - Intelligent Input Bus

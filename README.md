# Handy - Speech to Text for Fedora/GNOME

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)]()

Handy is a speech-to-text application for **Fedora Workstation on GNOME/Wayland**. It integrates natively with IBus, allowing you to dictate text into any application.

This is a fork of [Handy](https://github.com/cjpais/Handy) specifically optimized for Fedora Workstation with native IBus integration.

## Features

- **Native IBus Integration** - IBus engine + GNOME input source integration
- **Global Dictation Shortcut** - Uses system keyboard events for hands-free dictation
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

1. Configure your dictation shortcut in Handy preferences.
2. Press the shortcut once to start recording.
3. Press the shortcut again to stop recording and commit text.

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

1. **Daemon**: `handy --daemon` exports `com.handy.Transcription` on session D-Bus.
2. **Global shortcut listener**: daemon listens to evdev keyboard events for the configured shortcut.
3. **IBus bridge**: first press starts recording and next press stops; engine-side pending commit path commits text to the focused app.
4. **Transcription**: audio is transcribed locally; optional post-processing can rewrite final output.

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

### Push-to-talk does not trigger

```bash
systemctl --user status handy.service
systemctl --user restart handy.service
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

use crate::settings::{Settings, SoundTheme};
use cpal::traits::HostTrait;
use log::{debug, error, warn};
use rodio::OutputStreamBuilder;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::thread;

pub enum SoundType {
    Start,
    Stop,
}

fn get_sound_path(settings: &Settings, sound_type: SoundType) -> PathBuf {
    let filename = match (settings.sound_theme(), sound_type) {
        (SoundTheme::Custom, SoundType::Start) => "custom_start.wav",
        (SoundTheme::Custom, SoundType::Stop) => "custom_stop.wav",
        (_, SoundType::Start) => "marimba_start.wav",
        (_, SoundType::Stop) => "marimba_stop.wav",
    };

    let data_dir = std::env::var("XDG_DATA_HOME")
        .map(|p| PathBuf::from(p).join("handy").join("sounds"))
        .unwrap_or_else(|_| {
            PathBuf::from("/usr/share/handy/sounds")
        });

    if settings.sound_theme() == SoundTheme::Custom {
        data_dir.join(filename)
    } else {
        PathBuf::from("/usr/share/handy/sounds").join(filename)
    }
}

pub fn play_feedback_sound(settings: &Settings, sound_type: SoundType) {
    if !settings.audio_feedback() {
        return;
    }
    let path = get_sound_path(settings, sound_type);
    let settings = settings.clone();
    thread::spawn(move || {
        if let Err(e) = play_audio_file(&path, settings.selected_output_device().as_deref(), settings.audio_feedback_volume()) {
            error!("Failed to play sound '{}': {}", path.display(), e);
        }
    });
}

pub fn play_feedback_sound_blocking(settings: &Settings, sound_type: SoundType) {
    if !settings.audio_feedback() {
        return;
    }
    let path = get_sound_path(settings, sound_type);
    if let Err(e) = play_audio_file(&path, settings.selected_output_device().as_deref(), settings.audio_feedback_volume()) {
        error!("Failed to play sound '{}': {}", path.display(), e);
    }
}

pub fn play_test_sound(settings: &Settings, sound_type: SoundType) {
    let path = get_sound_path(settings, sound_type);
    if let Err(e) = play_audio_file(&path, settings.selected_output_device().as_deref(), settings.audio_feedback_volume()) {
        error!("Failed to play sound '{}': {}", path.display(), e);
    }
}

fn play_audio_file(
    path: &Path,
    selected_device: Option<&str>,
    volume: f32,
) -> Result<(), Box<dyn std::error::Error>> {
    let stream_builder = if let Some(device_name) = selected_device {
        if device_name == "Default" {
            debug!("Using default device");
            OutputStreamBuilder::from_default_device()?
        } else {
            let host = crate::audio_toolkit::get_cpal_host();
            let devices = host.output_devices()?;

            let mut found_device = None;
            for device in devices {
                if device.name()? == device_name {
                    found_device = Some(device);
                    break;
                }
            }

            match found_device {
                Some(device) => OutputStreamBuilder::from_device(device)?,
                None => {
                    warn!("Device '{}' not found, using default device", device_name);
                    OutputStreamBuilder::from_default_device()?
                }
            }
        }
    } else {
        debug!("Using default device");
        OutputStreamBuilder::from_default_device()?
    };

    let stream_handle = stream_builder.open_stream()?;
    let mixer = stream_handle.mixer();

    let file = File::open(path)?;
    let buf_reader = BufReader::new(file);

    let sink = rodio::play(mixer, buf_reader)?;
    sink.set_volume(volume);
    sink.sleep_until_end();

    Ok(())
}

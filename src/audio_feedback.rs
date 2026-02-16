use crate::settings::{Settings, SoundTheme};
use log::{debug, error};
use rodio::{OutputStream, Sink};
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
        .unwrap_or_else(|_| PathBuf::from("/usr/share/handy/sounds"));

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
    let volume = settings.audio_feedback_volume();
    thread::spawn(move || {
        if let Err(e) = play_audio_file(&path, volume) {
            error!("Failed to play sound '{}': {}", path.display(), e);
        }
    });
}

pub fn play_feedback_sound_blocking(settings: &Settings, sound_type: SoundType) {
    if !settings.audio_feedback() {
        return;
    }
    let path = get_sound_path(settings, sound_type);
    if let Err(e) = play_audio_file(&path, settings.audio_feedback_volume()) {
        error!("Failed to play sound '{}': {}", path.display(), e);
    }
}

pub fn play_test_sound(settings: &Settings, sound_type: SoundType) {
    let path = get_sound_path(settings, sound_type);
    if let Err(e) = play_audio_file(&path, settings.audio_feedback_volume()) {
        error!("Failed to play sound '{}': {}", path.display(), e);
    }
}

fn play_audio_file(
    path: &Path,
    volume: f32,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    debug!("Playing audio file: {}", path.display());

    let (_stream, stream_handle) = OutputStream::try_default()?;

    let file = File::open(path)?;
    let buf_reader = BufReader::new(file);
    let source = rodio::Decoder::new(buf_reader)?;

    let sink = Sink::try_new(&stream_handle)?;
    sink.append(source);
    sink.set_volume(volume);
    sink.sleep_until_end();

    Ok(())
}

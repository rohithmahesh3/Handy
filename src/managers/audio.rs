use crate::audio_toolkit::{list_input_devices, vad::SmoothedVad, AudioRecorder, SileroVad};
use log::{debug, error, info};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

const WHISPER_SAMPLE_RATE: usize = 16000;

#[derive(Clone, Debug)]
pub enum RecordingState {
    Idle,
    Recording { binding_id: String },
}

#[derive(Clone, Debug)]
pub enum MicrophoneMode {
    AlwaysOn,
    OnDemand,
}

pub struct AudioRecordingManager {
    state: Arc<Mutex<RecordingState>>,
    mode: Arc<Mutex<MicrophoneMode>>,
    selected_microphone: Arc<Mutex<Option<String>>>,
    mute_while_recording: Arc<Mutex<bool>>,
    recorder: Arc<Mutex<Option<AudioRecorder>>>,
    is_open: Arc<Mutex<bool>>,
    is_recording: Arc<Mutex<bool>>,
    did_mute: Arc<Mutex<bool>>,
}

fn set_mute(mute: bool) {
    use std::process::Command;

    let mute_val = if mute { "1" } else { "0" };
    let amixer_state = if mute { "mute" } else { "unmute" };

    if Command::new("wpctl")
        .args(["set-mute", "@DEFAULT_AUDIO_SINK@", mute_val])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        return;
    }

    if Command::new("pactl")
        .args(["set-sink-mute", "@DEFAULT_SINK@", mute_val])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        return;
    }

    let _ = Command::new("amixer")
        .args(["set", "Master", amixer_state])
        .output();
}

impl AudioRecordingManager {
    pub fn new() -> Result<Self, anyhow::Error> {
        let settings = crate::settings::Settings::new();
        let mode = if settings.always_on_microphone() {
            MicrophoneMode::AlwaysOn
        } else {
            MicrophoneMode::OnDemand
        };

        let manager = Self {
            state: Arc::new(Mutex::new(RecordingState::Idle)),
            mode: Arc::new(Mutex::new(mode.clone())),
            selected_microphone: Arc::new(Mutex::new(settings.selected_microphone())),
            mute_while_recording: Arc::new(Mutex::new(settings.mute_while_recording())),
            recorder: Arc::new(Mutex::new(None)),
            is_open: Arc::new(Mutex::new(false)),
            is_recording: Arc::new(Mutex::new(false)),
            did_mute: Arc::new(Mutex::new(false)),
        };

        if matches!(mode, MicrophoneMode::AlwaysOn) {
            if let Err(e) = manager.start_microphone_stream() {
                error!(
                    "Failed to start always-on microphone stream during initialization: {}. \
Falling back to on-demand mode.",
                    e
                );
                *manager.mode.lock().unwrap() = MicrophoneMode::OnDemand;
            }
        }

        Ok(manager)
    }

    fn get_effective_microphone_device(&self) -> Option<cpal::Device> {
        let device_name = self.selected_microphone.lock().unwrap().clone()?;

        match list_input_devices() {
            Ok(devices) => devices
                .into_iter()
                .find(|d| d.name == device_name)
                .map(|d| d.device),
            Err(e) => {
                debug!("Failed to list devices, using default: {}", e);
                None
            }
        }
    }

    pub fn apply_mute(&self) {
        let mut did_mute_guard = self.did_mute.lock().unwrap();

        if *self.mute_while_recording.lock().unwrap() && *self.is_open.lock().unwrap() {
            set_mute(true);
            *did_mute_guard = true;
            debug!("Mute applied");
        }
    }

    pub fn remove_mute(&self) {
        let mut did_mute_guard = self.did_mute.lock().unwrap();
        if *did_mute_guard {
            set_mute(false);
            *did_mute_guard = false;
            debug!("Mute removed");
        }
    }

    fn create_audio_recorder(&self) -> Result<AudioRecorder, anyhow::Error> {
        let vad_path = resolve_vad_model_path().ok_or_else(|| {
            anyhow::anyhow!(
                "Silero VAD model not found. Expected /usr/share/handy/models/silero_vad_v4.onnx \
or resources/models/silero_vad_v4.onnx"
            )
        })?;

        let silero = SileroVad::new(
            vad_path
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("Silero VAD model path contains invalid UTF-8"))?,
            0.3,
        )
        .map_err(|e| anyhow::anyhow!("Failed to create SileroVad: {}", e))?;
        let smoothed_vad = SmoothedVad::new(Box::new(silero), 15, 15, 2);

        let recorder = AudioRecorder::new()
            .map_err(|e| anyhow::anyhow!("Failed to create AudioRecorder: {}", e))?
            .with_vad(Box::new(smoothed_vad));

        Ok(recorder)
    }

    pub fn start_microphone_stream(&self) -> Result<(), anyhow::Error> {
        let mut open_flag = self.is_open.lock().unwrap();
        if *open_flag {
            debug!("Microphone stream already active");
            return Ok(());
        }

        let start_time = Instant::now();

        let mut did_mute_guard = self.did_mute.lock().unwrap();
        *did_mute_guard = false;

        let mut recorder_opt = self.recorder.lock().unwrap();

        if recorder_opt.is_none() {
            *recorder_opt = Some(self.create_audio_recorder()?);
        }

        let selected_device = self.get_effective_microphone_device();

        if let Some(rec) = recorder_opt.as_mut() {
            rec.open(selected_device)
                .map_err(|e| anyhow::anyhow!("Failed to open recorder: {}", e))?;
        }

        *open_flag = true;
        info!(
            "Microphone stream initialized in {:?}",
            start_time.elapsed()
        );
        Ok(())
    }

    pub fn stop_microphone_stream(&self) {
        let mut open_flag = self.is_open.lock().unwrap();
        if !*open_flag {
            return;
        }

        let mut did_mute_guard = self.did_mute.lock().unwrap();
        if *did_mute_guard {
            set_mute(false);
        }
        *did_mute_guard = false;

        if let Some(rec) = self.recorder.lock().unwrap().as_mut() {
            if *self.is_recording.lock().unwrap() {
                let _ = rec.stop();
                *self.is_recording.lock().unwrap() = false;
            }
            let _ = rec.close();
        }

        *open_flag = false;
        debug!("Microphone stream stopped");
    }

    pub fn update_mode(&self, new_mode: MicrophoneMode) -> Result<(), anyhow::Error> {
        let mode_guard = self.mode.lock().unwrap();
        let cur_mode = mode_guard.clone();

        match (cur_mode, &new_mode) {
            (MicrophoneMode::AlwaysOn, MicrophoneMode::OnDemand) => {
                if matches!(*self.state.lock().unwrap(), RecordingState::Idle) {
                    drop(mode_guard);
                    self.stop_microphone_stream();
                }
            }
            (MicrophoneMode::OnDemand, MicrophoneMode::AlwaysOn) => {
                drop(mode_guard);
                self.start_microphone_stream()?;
            }
            _ => {}
        }

        *self.mode.lock().unwrap() = new_mode;
        Ok(())
    }

    pub fn set_mode_from_settings(&self, always_on_microphone: bool) -> Result<(), anyhow::Error> {
        let mode = if always_on_microphone {
            MicrophoneMode::AlwaysOn
        } else {
            MicrophoneMode::OnDemand
        };
        self.update_mode(mode)
    }

    pub fn set_mute_while_recording(&self, value: bool) {
        *self.mute_while_recording.lock().unwrap() = value;
    }

    pub fn set_selected_microphone(&self, value: Option<String>) -> Result<(), anyhow::Error> {
        *self.selected_microphone.lock().unwrap() = value;
        self.update_selected_device()
    }

    pub fn try_start_recording(&self, binding_id: &str) -> bool {
        let mut state = self.state.lock().unwrap();

        if let RecordingState::Idle = *state {
            if matches!(*self.mode.lock().unwrap(), MicrophoneMode::OnDemand) {
                if let Err(e) = self.start_microphone_stream() {
                    error!("Failed to open microphone stream: {e}");
                    return false;
                }
            }

            if let Some(rec) = self.recorder.lock().unwrap().as_ref() {
                if rec.start().is_ok() {
                    *self.is_recording.lock().unwrap() = true;
                    *state = RecordingState::Recording {
                        binding_id: binding_id.to_string(),
                    };
                    debug!("Recording started for binding {binding_id}");
                    return true;
                }
            }
            error!("Recorder not available");
            false
        } else {
            false
        }
    }

    pub fn update_selected_device(&self) -> Result<(), anyhow::Error> {
        if *self.is_open.lock().unwrap() {
            self.stop_microphone_stream();
            self.start_microphone_stream()?;
        }
        Ok(())
    }

    pub fn stop_recording(&self, binding_id: &str) -> Option<Vec<f32>> {
        let mut state = self.state.lock().unwrap();

        match *state {
            RecordingState::Recording {
                binding_id: ref active,
            } if active == binding_id => {
                *state = RecordingState::Idle;
                drop(state);

                let samples = if let Some(rec) = self.recorder.lock().unwrap().as_ref() {
                    match rec.stop() {
                        Ok(buf) => buf,
                        Err(e) => {
                            error!("stop() failed: {e}");
                            Vec::new()
                        }
                    }
                } else {
                    error!("Recorder not available");
                    Vec::new()
                };

                *self.is_recording.lock().unwrap() = false;

                if matches!(*self.mode.lock().unwrap(), MicrophoneMode::OnDemand) {
                    self.stop_microphone_stream();
                }

                let s_len = samples.len();
                if s_len < WHISPER_SAMPLE_RATE && s_len > 0 {
                    let mut padded = samples;
                    padded.resize(WHISPER_SAMPLE_RATE * 5 / 4, 0.0);
                    Some(padded)
                } else {
                    Some(samples)
                }
            }
            _ => None,
        }
    }

    pub fn is_recording(&self) -> bool {
        matches!(
            *self.state.lock().unwrap(),
            RecordingState::Recording { .. }
        )
    }

    pub fn cancel_recording(&self) {
        let mut state = self.state.lock().unwrap();

        if let RecordingState::Recording { .. } = *state {
            *state = RecordingState::Idle;
            drop(state);

            if let Some(rec) = self.recorder.lock().unwrap().as_ref() {
                let _ = rec.stop();
            }

            *self.is_recording.lock().unwrap() = false;

            if matches!(*self.mode.lock().unwrap(), MicrophoneMode::OnDemand) {
                self.stop_microphone_stream();
            }
        }
    }
}

fn resolve_vad_model_path() -> Option<PathBuf> {
    let candidates = [
        PathBuf::from("/usr/share/handy/models/silero_vad_v4.onnx"),
        PathBuf::from("resources/models/silero_vad_v4.onnx"),
    ];

    candidates.into_iter().find(|p| p.exists())
}

use crate::{
    types::{
        audio_player::{FullState, PlayerState},
        config::HotkeyConfig,
        gui::CaptureSource,
        socket::{Request, Response},
    },
    utils::{
        commands::parse_command,
        daemon::get_audio_player,
        loudness::{calibrate_voice_capture, calibrate_voice_capture_until_stopped},
        pipewire::{get_all_devices, get_device},
    },
};
use async_trait::async_trait;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

static VOICE_CALIBRATION_ACTIVE: AtomicBool = AtomicBool::new(false);
static VOICE_CALIBRATION_STOP: OnceLock<Arc<AtomicBool>> = OnceLock::new();

fn voice_calibration_stop_flag() -> Arc<AtomicBool> {
    VOICE_CALIBRATION_STOP
        .get_or_init(|| Arc::new(AtomicBool::new(false)))
        .clone()
}

#[async_trait]
pub trait Executable {
    async fn execute(&self) -> Response;
}

pub struct PingCommand {}

pub struct KillCommand {}

pub struct PauseCommand {
    pub id: Option<u32>,
}

pub struct ResumeCommand {
    pub id: Option<u32>,
}

pub struct TogglePauseCommand {
    pub id: Option<u32>,
}

pub struct StopCommand {
    pub id: Option<u32>,
}

pub struct IsPausedCommand {}

pub struct GetStateCommand {}

pub struct GetVolumeCommand {
    pub id: Option<u32>,
}

pub struct SetVolumeCommand {
    pub volume: Option<f32>,
    pub id: Option<u32>,
}

pub struct GetPositionCommand {
    pub id: Option<u32>,
}

pub struct SeekCommand {
    pub position: Option<f32>,
    pub id: Option<u32>,
}

pub struct GetDurationCommand {
    pub id: Option<u32>,
}

pub struct PlayCommand {
    pub file_path: Option<PathBuf>,
    pub concurrent: Option<bool>,
}

pub struct GetTracksCommand {}

pub struct GetCurrentInputCommand {}

pub struct GetAllInputsCommand {}

pub struct SetCurrentInputCommand {
    pub name: Option<String>,
}

pub struct SetLoopCommand {
    pub enabled: Option<bool>,
    pub id: Option<u32>,
}

pub struct ToggleLoopCommand {
    pub id: Option<u32>,
}

pub struct GetDaemonVersionCommand {}

pub struct GetFullStateCommand {}

pub struct GetHotkeysCommand {}

pub struct GetNormalizationConfigCommand {}

pub struct SetNormalizationConfigCommand {
    pub enabled: Option<bool>,
    pub calibration_device_name: Option<String>,
}

pub struct GetCaptureSourcesCommand {}

pub struct CalibrateVoiceCommand {
    pub device_name: Option<String>,
    pub duration_secs: Option<u32>,
}

pub struct StopVoiceCalibrationCommand {}

pub struct SetHotkeyCommand {
    pub slot: Option<String>,
    pub file_path: Option<PathBuf>,
}

pub struct SetHotkeyActionCommand {
    pub slot: Option<String>,
    pub action: Option<Request>,
}

pub struct SetHotkeyKeyCommand {
    pub slot: Option<String>,
    pub key_chord: Option<String>,
}

pub struct SetHotkeyActionAndKeyCommand {
    pub slot: Option<String>,
    pub action: Option<Request>,
    pub key_chord: Option<String>,
}

pub struct PlayHotkeyCommand {
    pub slot: Option<String>,
}

pub struct ClearHotkeyCommand {
    pub slot: Option<String>,
}

pub struct ClearHotkeyKeyCommand {
    pub slot: Option<String>,
}

#[async_trait]
impl Executable for PingCommand {
    async fn execute(&self) -> Response {
        Response::new(true, "pong")
    }
}

#[async_trait]
impl Executable for KillCommand {
    async fn execute(&self) -> Response {
        Response::new(true, "killed")
    }
}

#[async_trait]
impl Executable for PauseCommand {
    async fn execute(&self) -> Response {
        let mut audio_player = match get_audio_player().await {
            Ok(player) => player.lock().await,
            Err(err) => return Response::new(false, format!("Audio player error: {}", err)),
        };
        audio_player.pause(self.id);
        Response::new(true, "Audio was paused")
    }
}

#[async_trait]
impl Executable for ResumeCommand {
    async fn execute(&self) -> Response {
        let mut audio_player = match get_audio_player().await {
            Ok(player) => player.lock().await,
            Err(err) => return Response::new(false, format!("Audio player error: {}", err)),
        };
        audio_player.resume(self.id);
        Response::new(true, "Audio was resumed")
    }
}

#[async_trait]
impl Executable for TogglePauseCommand {
    async fn execute(&self) -> Response {
        let mut audio_player = match get_audio_player().await {
            Ok(player) => player.lock().await,
            Err(err) => return Response::new(false, format!("Audio player error: {}", err)),
        };

        if audio_player.get_state() == PlayerState::Stopped {
            return Response::new(false, "Audio is not playing");
        }

        // This logic is a bit tricky with multiple tracks.
        // If ID is provided, toggle that track.
        // If not, toggle global pause state?
        // For now, let's just use pause/resume based on global state if no ID.

        if let Some(id) = self.id {
            if let Some(track) = audio_player.tracks.get(&id) {
                if track.sink.is_paused() {
                    audio_player.resume(Some(id));
                    Response::new(true, "Audio was resumed")
                } else {
                    audio_player.pause(Some(id));
                    Response::new(true, "Audio was paused")
                }
            } else {
                Response::new(false, "Track not found")
            }
        } else {
            if audio_player.is_paused() {
                audio_player.resume(None);
                Response::new(true, "Audio was resumed")
            } else {
                audio_player.pause(None);
                Response::new(true, "Audio was paused")
            }
        }
    }
}

#[async_trait]
impl Executable for StopCommand {
    async fn execute(&self) -> Response {
        let mut audio_player = match get_audio_player().await {
            Ok(player) => player.lock().await,
            Err(err) => return Response::new(false, format!("Audio player error: {}", err)),
        };
        audio_player.stop(self.id);
        Response::new(true, "Audio was stopped")
    }
}

#[async_trait]
impl Executable for IsPausedCommand {
    async fn execute(&self) -> Response {
        let audio_player = match get_audio_player().await {
            Ok(player) => player.lock().await,
            Err(err) => return Response::new(false, format!("Audio player error: {}", err)),
        };
        let is_paused = audio_player.is_paused().to_string();
        Response::new(true, is_paused)
    }
}

#[async_trait]
impl Executable for GetStateCommand {
    async fn execute(&self) -> Response {
        let audio_player = match get_audio_player().await {
            Ok(player) => player.lock().await,
            Err(err) => return Response::new(false, format!("Audio player error: {}", err)),
        };
        let state = audio_player.get_state();
        match serde_json::to_string(&state) {
            Ok(json) => Response::new(true, json),
            Err(err) => Response::new(false, format!("Failed to serialize state: {}", err)),
        }
    }
}

#[async_trait]
impl Executable for GetVolumeCommand {
    async fn execute(&self) -> Response {
        let mut audio_player = match get_audio_player().await {
            Ok(player) => player.lock().await,
            Err(err) => return Response::new(false, format!("Audio player error: {}", err)),
        };
        let volume = audio_player.get_volume(self.id);

        if let Some(volume) = volume {
            Response::new(true, volume.to_string())
        } else {
            Response::new(false, "Failed to get volume")
        }
    }
}

#[async_trait]
impl Executable for SetVolumeCommand {
    async fn execute(&self) -> Response {
        if let Some(volume) = self.volume {
            let mut audio_player = match get_audio_player().await {
                Ok(player) => player.lock().await,
                Err(err) => return Response::new(false, format!("Audio player error: {}", err)),
            };
            audio_player.set_volume(volume, self.id);
            Response::new(true, format!("Audio volume was set to {}", volume))
        } else {
            Response::new(false, "Invalid volume value")
        }
    }
}

#[async_trait]
impl Executable for GetPositionCommand {
    async fn execute(&self) -> Response {
        let audio_player = match get_audio_player().await {
            Ok(player) => player.lock().await,
            Err(err) => return Response::new(false, format!("Audio player error: {}", err)),
        };
        let position = audio_player.get_position(self.id);
        Response::new(true, position.to_string())
    }
}

#[async_trait]
impl Executable for SeekCommand {
    async fn execute(&self) -> Response {
        if let Some(position) = self.position {
            let mut audio_player = match get_audio_player().await {
                Ok(player) => player.lock().await,
                Err(err) => return Response::new(false, format!("Audio player error: {}", err)),
            };
            match audio_player.seek(position, self.id) {
                Ok(_) => Response::new(true, format!("Audio position was set to {}", position)),
                Err(err) => Response::new(false, err.to_string()),
            }
        } else {
            Response::new(false, "Invalid position value")
        }
    }
}

#[async_trait]
impl Executable for GetDurationCommand {
    async fn execute(&self) -> Response {
        let mut audio_player = match get_audio_player().await {
            Ok(player) => player.lock().await,
            Err(err) => return Response::new(false, format!("Audio player error: {}", err)),
        };
        match audio_player.get_duration(self.id) {
            Ok(duration) => Response::new(true, duration.to_string()),
            Err(err) => Response::new(false, err.to_string()),
        }
    }
}

#[async_trait]
impl Executable for PlayCommand {
    async fn execute(&self) -> Response {
        if let Some(file_path) = &self.file_path {
            let mut audio_player = match get_audio_player().await {
                Ok(player) => player.lock().await,
                Err(err) => return Response::new(false, format!("Audio player error: {}", err)),
            };
            match audio_player
                .play(file_path, self.concurrent.unwrap_or(false))
                .await
            {
                Ok(id) => Response::new(true, id.to_string()),
                Err(err) => Response::new(false, err.to_string()),
            }
        } else {
            Response::new(false, "Invalid file path")
        }
    }
}

#[async_trait]
impl Executable for GetTracksCommand {
    async fn execute(&self) -> Response {
        let audio_player = match get_audio_player().await {
            Ok(player) => player.lock().await,
            Err(err) => return Response::new(false, format!("Audio player error: {}", err)),
        };
        let tracks = audio_player.get_tracks();
        match serde_json::to_string(&tracks) {
            Ok(json) => Response::new(true, json),
            Err(err) => Response::new(false, format!("Failed to serialize tracks: {}", err)),
        }
    }
}

#[async_trait]
impl Executable for GetCurrentInputCommand {
    async fn execute(&self) -> Response {
        let audio_player = match get_audio_player().await {
            Ok(player) => player.lock().await,
            Err(err) => return Response::new(false, format!("Audio player error: {}", err)),
        };
        if let Some(input_device_name) = &audio_player.input_device_name {
            if let Ok(input_device) = get_device(input_device_name).await {
                Response::new(
                    true,
                    format!("{} - {}", input_device.name, input_device.nick),
                )
            } else {
                Response::new(false, "Failed to get current input device")
            }
        } else {
            Response::new(false, "No input device selected")
        }
    }
}

#[async_trait]
impl Executable for GetAllInputsCommand {
    async fn execute(&self) -> Response {
        let (input_devices, _output_devices) = match get_all_devices().await {
            Ok(devices) => devices,
            Err(err) => return Response::new(false, format!("Failed to get devices: {}", err)),
        };
        let mut input_devices_strings = vec![];
        for device in input_devices {
            if device.name == "pwsp-virtual-mic" {
                continue;
            }

            let string = format!("{} - {}", device.name, device.nick);
            input_devices_strings.push(string);
        }
        let response_message = input_devices_strings.join("; ");

        Response::new(true, response_message)
    }
}

#[async_trait]
impl Executable for SetCurrentInputCommand {
    async fn execute(&self) -> Response {
        if let Some(name) = &self.name {
            let mut audio_player = match get_audio_player().await {
                Ok(player) => player.lock().await,
                Err(err) => return Response::new(false, format!("Audio player error: {}", err)),
            };
            match audio_player.set_current_input_device(name).await {
                Ok(_) => Response::new(true, "Input device was set"),
                Err(err) => Response::new(false, err.to_string()),
            }
        } else {
            Response::new(false, "Invalid index value")
        }
    }
}

#[async_trait]
impl Executable for SetLoopCommand {
    async fn execute(&self) -> Response {
        let mut audio_player = match get_audio_player().await {
            Ok(player) => player.lock().await,
            Err(err) => return Response::new(false, format!("Audio player error: {}", err)),
        };

        match self.enabled {
            Some(enabled) => {
                audio_player.set_loop(enabled, self.id);
                Response::new(true, format!("Loop was set to {}", enabled))
            }
            None => Response::new(false, "Invalid enabled value"),
        }
    }
}

#[async_trait]
impl Executable for ToggleLoopCommand {
    async fn execute(&self) -> Response {
        let mut audio_player = match get_audio_player().await {
            Ok(player) => player.lock().await,
            Err(err) => return Response::new(false, format!("Audio player error: {}", err)),
        };
        if let Some(id) = self.id {
            if let Some(track) = audio_player.tracks.get_mut(&id) {
                track.looped = !track.looped;
                Response::new(true, format!("Loop was set to {}", track.looped))
            } else {
                Response::new(false, "Track not found")
            }
        } else {
            // Toggle all?
            for track in audio_player.tracks.values_mut() {
                track.looped = !track.looped;
            }
            Response::new(true, "Loop toggled for all tracks")
        }
    }
}

#[async_trait]
impl Executable for GetDaemonVersionCommand {
    async fn execute(&self) -> Response {
        Response::new(true, env!("CARGO_PKG_VERSION"))
    }
}

#[async_trait]
impl Executable for GetFullStateCommand {
    async fn execute(&self) -> Response {
        let (input_devices, _output_devices) = match get_all_devices().await {
            Ok(devices) => devices,
            Err(err) => return Response::new(false, format!("Failed to get devices: {}", err)),
        };
        let mut all_inputs = HashMap::new();
        let mut current_input_nick = String::new();

        let audio_player = match get_audio_player().await {
            Ok(player) => player.lock().await,
            Err(err) => return Response::new(false, format!("Audio player error: {}", err)),
        };
        if let Some(current_input_name) = &audio_player.input_device_name {
            for device in input_devices {
                if device.name == "pwsp-virtual-mic" {
                    continue;
                }
                if device.name == *current_input_name {
                    current_input_nick = format!("{} - {}", device.name, device.nick);
                }

                all_inputs.insert(device.name, device.nick);
            }
        } else {
            for device in input_devices {
                if device.name == "pwsp-virtual-mic" {
                    continue;
                }

                all_inputs.insert(device.name, device.nick);
            }
        }

        let full_state = FullState {
            state: audio_player.get_state(),
            tracks: audio_player.get_tracks(),
            volume: audio_player.volume,
            current_input: current_input_nick,
            all_inputs,
        };

        match serde_json::to_string(&full_state) {
            Ok(json) => Response::new(true, json),
            Err(err) => Response::new(false, format!("Failed to serialize full state: {}", err)),
        }
    }
}

#[async_trait]
impl Executable for GetHotkeysCommand {
    async fn execute(&self) -> Response {
        match HotkeyConfig::load() {
            Ok(config) => match serde_json::to_string(&config) {
                Ok(json) => Response::new(true, json),
                Err(err) => Response::new(false, format!("Failed to serialize hotkeys: {}", err)),
            },
            Err(err) => Response::new(false, format!("Failed to load hotkeys: {}", err)),
        }
    }
}

#[async_trait]
impl Executable for GetNormalizationConfigCommand {
    async fn execute(&self) -> Response {
        let config = crate::utils::daemon::get_daemon_config().normalization;
        match serde_json::to_string(&config) {
            Ok(json) => Response::new(true, json),
            Err(err) => Response::new(false, format!("Failed to serialize normalization: {}", err)),
        }
    }
}

#[async_trait]
impl Executable for SetNormalizationConfigCommand {
    async fn execute(&self) -> Response {
        let Some(enabled) = self.enabled else {
            return Response::new(false, "Missing enabled value");
        };

        let mut daemon_config = crate::utils::daemon::get_daemon_config();
        daemon_config.normalization.enabled = enabled;
        if let Some(device_name) = &self.calibration_device_name {
            daemon_config.normalization.calibration_device_name = Some(device_name.clone());
        }

        if let Err(err) = daemon_config.save_to_file() {
            return Response::new(
                false,
                format!("Failed to save normalization config: {}", err),
            );
        }

        let mut audio_player = match get_audio_player().await {
            Ok(player) => player.lock().await,
            Err(err) => return Response::new(false, format!("Audio player error: {}", err)),
        };
        audio_player.refresh_normalization_config();

        Response::new(true, "Normalization config updated")
    }
}

#[async_trait]
impl Executable for GetCaptureSourcesCommand {
    async fn execute(&self) -> Response {
        let (input_devices, _output_devices) = match get_all_devices().await {
            Ok(devices) => devices,
            Err(err) => return Response::new(false, format!("Failed to get devices: {}", err)),
        };

        let sources: Vec<CaptureSource> = input_devices
            .into_iter()
            .filter(|device| device.name != "pwsp-virtual-mic")
            .filter(|device| device.output_fl.is_some() || device.output_fr.is_some())
            .map(|device| CaptureSource {
                label: if device.nick.is_empty() {
                    device.name.clone()
                } else {
                    device.nick
                },
                name: device.name,
            })
            .collect();

        match serde_json::to_string(&sources) {
            Ok(json) => Response::new(true, json),
            Err(err) => Response::new(
                false,
                format!("Failed to serialize capture sources: {}", err),
            ),
        }
    }
}

#[async_trait]
impl Executable for CalibrateVoiceCommand {
    async fn execute(&self) -> Response {
        if VOICE_CALIBRATION_ACTIVE.swap(true, Ordering::SeqCst) {
            return Response::new(false, "Voice calibration is already running");
        }

        let stop_requested = voice_calibration_stop_flag();
        stop_requested.store(false, Ordering::SeqCst);

        let selected_device_name = self.device_name.clone();
        let capture_device_hint = match selected_device_name.as_deref() {
            Some(device_name) => match get_device(device_name).await {
                Ok(device) if !device.nick.is_empty() => Some(device.nick),
                _ => Some(device_name.to_string()),
            },
            None => None,
        };

        let calibration_result = if let Some(duration_secs) = self.duration_secs {
            calibrate_voice_capture(capture_device_hint.as_deref(), duration_secs.clamp(1, 15))
        } else {
            calibrate_voice_capture_until_stopped(capture_device_hint.as_deref(), stop_requested)
        }
        .map_err(|err| err.to_string());
        VOICE_CALIBRATION_ACTIVE.store(false, Ordering::SeqCst);

        let mut calibration = match calibration_result {
            Ok(calibration) => calibration,
            Err(err) => return Response::new(false, format!("Voice calibration failed: {}", err)),
        };
        if let Some(device_name) = selected_device_name {
            calibration.device_name = Some(device_name);
        }

        let mut daemon_config = crate::utils::daemon::get_daemon_config();
        daemon_config.normalization.calibrated_voice_lufs = Some(calibration.lufs);
        if let Some(device_name) = &calibration.device_name {
            daemon_config.normalization.calibration_device_name = Some(device_name.clone());
        }
        if let Err(err) = daemon_config.save_to_file() {
            return Response::new(false, format!("Failed to save calibration result: {}", err));
        }

        let mut audio_player = match get_audio_player().await {
            Ok(player) => player.lock().await,
            Err(err) => return Response::new(false, format!("Audio player error: {}", err)),
        };
        audio_player.refresh_normalization_config();

        match serde_json::to_string(&calibration) {
            Ok(json) => Response::new(true, json),
            Err(err) => Response::new(false, format!("Failed to serialize calibration: {}", err)),
        }
    }
}

#[async_trait]
impl Executable for StopVoiceCalibrationCommand {
    async fn execute(&self) -> Response {
        if !VOICE_CALIBRATION_ACTIVE.load(Ordering::SeqCst) {
            return Response::new(false, "Voice calibration is not running");
        }

        voice_calibration_stop_flag().store(true, Ordering::SeqCst);
        Response::new(true, "Stopping voice calibration")
    }
}

#[async_trait]
impl Executable for SetHotkeyCommand {
    async fn execute(&self) -> Response {
        let Some(slot) = &self.slot else {
            return Response::new(false, "Missing slot name");
        };
        let Some(file_path) = &self.file_path else {
            return Response::new(false, "Missing file path");
        };

        let mut config = match HotkeyConfig::load() {
            Ok(c) => c,
            Err(err) => return Response::new(false, format!("Failed to load hotkeys: {}", err)),
        };

        config.set_slot(
            slot.clone(),
            Request::play(&file_path.to_string_lossy(), false),
        );

        match config.save() {
            Ok(_) => Response::new(true, format!("Hotkey slot '{}' set", slot)),
            Err(err) => Response::new(false, format!("Failed to save hotkeys: {}", err)),
        }
    }
}

#[async_trait]
impl Executable for SetHotkeyActionCommand {
    async fn execute(&self) -> Response {
        let Some(slot) = &self.slot else {
            return Response::new(false, "Missing slot name");
        };
        let Some(action) = &self.action else {
            return Response::new(false, "Missing or invalid action");
        };

        let mut config = match HotkeyConfig::load() {
            Ok(c) => c,
            Err(err) => return Response::new(false, format!("Failed to load hotkeys: {}", err)),
        };

        config.set_slot(slot.clone(), action.clone());

        match config.save() {
            Ok(_) => Response::new(true, format!("Hotkey slot '{}' set", slot)),
            Err(err) => Response::new(false, format!("Failed to save hotkeys: {}", err)),
        }
    }
}

#[async_trait]
impl Executable for SetHotkeyKeyCommand {
    async fn execute(&self) -> Response {
        let Some(slot) = &self.slot else {
            return Response::new(false, "Missing slot name");
        };
        let Some(key_chord) = &self.key_chord else {
            return Response::new(false, "Missing key chord");
        };

        let mut config = match HotkeyConfig::load() {
            Ok(c) => c,
            Err(err) => return Response::new(false, format!("Failed to load hotkeys: {}", err)),
        };

        if !config.set_key_chord(slot, Some(key_chord.clone())) {
            return Response::new(false, format!("Slot '{}' not found", slot));
        }

        match config.save() {
            Ok(_) => Response::new(
                true,
                format!("Key chord for slot '{}' set to '{}'", slot, key_chord),
            ),
            Err(err) => Response::new(false, format!("Failed to save hotkeys: {}", err)),
        }
    }
}

#[async_trait]
impl Executable for SetHotkeyActionAndKeyCommand {
    async fn execute(&self) -> Response {
        let Some(slot) = &self.slot else {
            return Response::new(false, "Missing slot name");
        };
        let Some(action) = &self.action else {
            return Response::new(false, "Missing or invalid action");
        };
        let Some(key_chord) = &self.key_chord else {
            return Response::new(false, "Missing key chord");
        };

        let mut config = match HotkeyConfig::load() {
            Ok(c) => c,
            Err(err) => return Response::new(false, format!("Failed to load hotkeys: {}", err)),
        };

        // Set the action and then the key chord
        config.set_slot(slot.clone(), action.clone());
        if !config.set_key_chord(slot, Some(key_chord.clone())) {
            return Response::new(
                false,
                format!("Slot '{}' not found after setting action", slot),
            );
        }

        match config.save() {
            Ok(_) => Response::new(
                true,
                format!(
                    "Hotkey slot '{}' set with action and key chord '{}'",
                    slot, key_chord
                ),
            ),
            Err(err) => Response::new(false, format!("Failed to save hotkeys: {}", err)),
        }
    }
}

#[async_trait]
impl Executable for PlayHotkeyCommand {
    async fn execute(&self) -> Response {
        let Some(slot) = &self.slot else {
            return Response::new(false, "Missing slot name");
        };

        let config = match HotkeyConfig::load() {
            Ok(c) => c,
            Err(err) => return Response::new(false, format!("Failed to load hotkeys: {}", err)),
        };

        let Some(hotkey_slot) = config.find_slot(slot) else {
            return Response::new(false, format!("Slot '{}' not found", slot));
        };

        let action = hotkey_slot.action.clone();

        if let Some(cmd) = parse_command(&action) {
            cmd.execute().await
        } else {
            Response::new(false, "Unknown command in hotkey slot")
        }
    }
}

#[async_trait]
impl Executable for ClearHotkeyCommand {
    async fn execute(&self) -> Response {
        let Some(slot) = &self.slot else {
            return Response::new(false, "Missing slot name");
        };

        let mut config = match HotkeyConfig::load() {
            Ok(c) => c,
            Err(err) => return Response::new(false, format!("Failed to load hotkeys: {}", err)),
        };

        if config.remove_slot(slot) {
            match config.save() {
                Ok(_) => Response::new(true, format!("Hotkey slot '{}' cleared", slot)),
                Err(err) => Response::new(false, format!("Failed to save hotkeys: {}", err)),
            }
        } else {
            Response::new(false, format!("Slot '{}' not found", slot))
        }
    }
}

#[async_trait]
impl Executable for ClearHotkeyKeyCommand {
    async fn execute(&self) -> Response {
        let Some(slot) = &self.slot else {
            return Response::new(false, "Missing slot name");
        };

        let mut config = match HotkeyConfig::load() {
            Ok(c) => c,
            Err(err) => return Response::new(false, format!("Failed to load hotkeys: {}", err)),
        };

        if !config.set_key_chord(slot, None) {
            return Response::new(false, format!("Slot '{}' not found", slot));
        }

        match config.save() {
            Ok(_) => Response::new(true, format!("Key chord for slot '{}' cleared", slot)),
            Err(err) => Response::new(false, format!("Failed to save hotkeys: {}", err)),
        }
    }
}

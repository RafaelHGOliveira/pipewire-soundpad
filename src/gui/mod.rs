mod draw;
mod input;
mod update;

use eframe::{HardwareAcceleration, NativeOptions, icon_data::from_png_bytes, run_native};
use egui::{Context, Vec2, ViewportBuilder};
use itertools::Itertools;
use pwsp::{
    types::{
        audio_player::PlayerState,
        config::{GuiConfig, HotkeyConfig, NormalizationConfig},
        gui::{AppState, AudioPlayerState, CalibrationUiResult, CaptureSource},
        socket::Request,
    },
    utils::{
        daemon::{get_daemon_config, make_request},
        gui::{get_gui_config, make_request_async, make_request_sync, start_app_state_thread},
    },
};
use rfd::FileDialog;
use std::{
    error::Error,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
};

const SUPPORTED_EXTENSIONS: [&str; 13] = [
    "mp3", "wav", "ogg", "flac", "mp4", "m4a", "aac", "mov", "mkv", "mka", "webm", "avi", "opus",
];

struct SoundpadGui {
    pub app_state: AppState,
    pub config: GuiConfig,
    pub audio_player_state: AudioPlayerState,
    pub audio_player_state_shared: Arc<Mutex<AudioPlayerState>>,
}

impl SoundpadGui {
    fn new(ctx: &Context) -> Self {
        let audio_player_state = Arc::new(Mutex::new(AudioPlayerState::default()));
        start_app_state_thread(audio_player_state.clone());

        let config = get_gui_config();

        ctx.set_zoom_factor(config.scale_factor);

        let mut soundpad_gui = SoundpadGui {
            app_state: AppState::default(),
            config: config.clone(),
            audio_player_state: AudioPlayerState::default(),
            audio_player_state_shared: audio_player_state.clone(),
        };

        soundpad_gui.app_state.dirs = config.dirs;
        soundpad_gui.app_state.hotkey_config = HotkeyConfig::load().unwrap_or_default();
        if let Some(last_dir) = &config.last_dir
            && last_dir.is_dir()
            && soundpad_gui.app_state.dirs.contains(last_dir)
        {
            soundpad_gui.open_dir(last_dir);
        }

        soundpad_gui
    }

    pub fn play_toggle(&mut self) {
        let (new_state, request) = {
            let guard = self
                .audio_player_state_shared
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            match guard.state {
                PlayerState::Playing => (Some(PlayerState::Paused), Some(Request::pause(None))),
                PlayerState::Paused => (Some(PlayerState::Playing), Some(Request::resume(None))),
                PlayerState::Stopped => (None, None),
            }
        };

        if let Some(req) = request {
            make_request_async(req);
        }

        if let Some(state) = new_state {
            let mut guard = self
                .audio_player_state_shared
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            guard.new_state = Some(state.clone());
            guard.state = state;
        }
    }

    pub fn open_file(&mut self) {
        let file_dialog = FileDialog::new().add_filter("Audio File", &SUPPORTED_EXTENSIONS);
        if let Some(path) = file_dialog.pick_file() {
            self.play_file(&path, false);
        }
    }

    pub fn add_dirs(&mut self) {
        let file_dialog = FileDialog::new();
        if let Some(paths) = file_dialog.pick_folders() {
            for path in paths {
                self.app_state.dirs.push(path);
            }
            self.app_state.dirs = self.app_state.dirs.iter().unique().cloned().collect();
            self.config.dirs = self.app_state.dirs.clone();
            self.config.save_to_file().ok();
        }
    }

    pub fn open_dir(&mut self, path: &PathBuf) {
        self.app_state.current_dir = Some(path.clone());
        self.config.last_dir = Some(path.clone());
        self.config.save_to_file().ok();
        match path.read_dir() {
            Ok(read_dir) => {
                self.app_state.files = read_dir
                    .filter_map(|res| res.ok())
                    .map(|entry| entry.path())
                    .collect();
            }
            Err(e) => {
                eprintln!("Failed to read directory {:?}: {}", path, e);
                self.app_state.files.clear();
            }
        }
    }

    pub fn play_file(&mut self, path: &Path, concurrent: bool) {
        make_request_async(Request::play(&path.to_string_lossy(), concurrent));
    }

    pub fn open_settings(&mut self) {
        self.app_state.show_settings = true;
        self.load_normalization_settings();
    }

    pub fn set_input(&mut self, name: String) {
        make_request_async(Request::set_input(&name));

        if self.config.save_input {
            let mut daemon_config = get_daemon_config();
            daemon_config.default_input_name = Some(name);
            daemon_config.save_to_file().ok();
        }
    }

    pub fn toggle_loop(&mut self, id: Option<u32>) {
        make_request_async(Request::toggle_loop(id));
    }

    pub fn pause(&mut self, id: Option<u32>) {
        make_request_async(Request::pause(id));
    }

    pub fn resume(&mut self, id: Option<u32>) {
        make_request_async(Request::resume(id));
    }

    pub fn stop(&mut self, id: Option<u32>) {
        make_request_async(Request::stop(id));
    }

    pub fn play_hotkey_slot(&mut self, slot: &str) {
        make_request_async(Request::play_hotkey(slot));
    }

    pub fn get_filtered_files(&self) -> Vec<PathBuf> {
        let mut files: Vec<PathBuf> = self.app_state.files.iter().cloned().collect();
        files.sort();

        let search_query = self.app_state.search_query.to_lowercase();
        let search_query = search_query.trim();

        files
            .into_iter()
            .filter(|entry_path| {
                if entry_path.is_dir() {
                    return false;
                }

                if !SUPPORTED_EXTENSIONS.contains(
                    &entry_path
                        .extension()
                        .unwrap_or_default()
                        .to_str()
                        .unwrap_or_default(),
                ) {
                    return false;
                }

                if !search_query.is_empty() {
                    let file_name = entry_path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();

                    if !file_name.to_lowercase().contains(search_query) {
                        return false;
                    }
                }

                true
            })
            .collect()
    }

    pub fn load_normalization_settings(&mut self) {
        let config_res = make_request_sync(Request::get_normalization_config());
        match config_res {
            Ok(res) if res.status => {
                let Ok(config) = serde_json::from_str::<NormalizationConfig>(&res.message) else {
                    self.app_state.normalization_ui.calibration_status =
                        Some("Failed to load normalization settings".to_string());
                    self.app_state.normalization_ui.loaded = true;
                    return;
                };
                self.app_state.normalization_ui.selected_capture_source =
                    config.calibration_device_name.clone().unwrap_or_default();
                self.app_state.normalization_ui.config = config;
                self.app_state.normalization_ui.supported = true;
            }
            Ok(res) if res.message == "Unknown command" => {
                self.app_state.normalization_ui.supported = false;
                self.app_state.normalization_ui.calibration_status = Some(
                    "Volume normalization requires restarting pwsp-daemon from this build"
                        .to_string(),
                );
                self.app_state.normalization_ui.loaded = true;
                return;
            }
            Ok(res) => {
                self.app_state.normalization_ui.supported = false;
                self.app_state.normalization_ui.calibration_status = Some(res.message);
                self.app_state.normalization_ui.loaded = true;
                return;
            }
            Err(err) => {
                self.app_state.normalization_ui.supported = false;
                self.app_state.normalization_ui.calibration_status = Some(err.to_string());
                self.app_state.normalization_ui.loaded = true;
                return;
            }
        }

        let sources_res = make_request_sync(Request::get_capture_sources());
        if let Ok(res) = sources_res
            && res.status
            && let Ok(sources) = serde_json::from_str::<Vec<CaptureSource>>(&res.message)
        {
            if self
                .app_state
                .normalization_ui
                .selected_capture_source
                .is_empty()
                || !sources.iter().any(|source| {
                    source.name == self.app_state.normalization_ui.selected_capture_source
                })
            {
                self.app_state.normalization_ui.selected_capture_source = sources
                    .first()
                    .map(|source| source.name.clone())
                    .unwrap_or_default();
            }
            self.app_state.normalization_ui.capture_sources = sources;
        }

        self.app_state.normalization_ui.loaded = true;
    }

    pub fn save_normalization_settings(&mut self) {
        let ui = &self.app_state.normalization_ui;
        if !ui.supported {
            return;
        }

        let device_name = if ui.selected_capture_source.is_empty() {
            None
        } else {
            Some(ui.selected_capture_source.as_str())
        };

        make_request_async(Request::set_normalization_config(
            ui.config.enabled,
            device_name,
        ));
    }

    pub fn calibrate_voice(&mut self) {
        if !self.app_state.normalization_ui.supported {
            return;
        }

        if self
            .app_state
            .normalization_ui
            .calibration_receiver
            .is_some()
        {
            self.app_state.normalization_ui.calibration_status =
                Some("Stopping voice calibration...".to_string());
            make_request_async(Request::stop_voice_calibration());
            return;
        }

        let device_name = if self
            .app_state
            .normalization_ui
            .selected_capture_source
            .is_empty()
        {
            None
        } else {
            Some(
                self.app_state
                    .normalization_ui
                    .selected_capture_source
                    .as_str(),
            )
        };

        let request = Request::start_voice_calibration(device_name);
        let (sender, receiver) = mpsc::channel();
        self.app_state.normalization_ui.calibration_status =
            Some("Calibrating voice... click Stop when finished".to_string());
        self.app_state.normalization_ui.calibration_receiver = Some(receiver);

        tokio::spawn(async move {
            let result = match make_request(request).await {
                Ok(response) if response.status => {
                    serde_json::from_str::<CalibrationUiResult>(&response.message)
                        .map_err(|_| "Calibration complete".to_string())
                }
                Ok(response) => Err(response.message),
                Err(err) => Err(err.to_string()),
            };

            sender.send(result).ok();
        });
    }

    pub fn poll_voice_calibration(&mut self) {
        let Some(receiver) = &self.app_state.normalization_ui.calibration_receiver else {
            return;
        };

        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Err("Voice calibration failed".to_string()),
        };

        self.app_state.normalization_ui.calibration_receiver = None;
        match result {
            Ok(result) => {
                self.app_state.normalization_ui.config.calibrated_voice_lufs = Some(result.lufs);
                if let Some(device_name) = result.device_name {
                    self.app_state.normalization_ui.selected_capture_source = device_name;
                }
                self.app_state.normalization_ui.calibration_status = Some(format!(
                    "Voice calibrated at {:.1} LUFS, peak {:.1} dBFS",
                    result.lufs, result.peak_dbfs
                ));
            }
            Err(status) => {
                self.app_state.normalization_ui.calibration_status = Some(status);
            }
        }
    }
}

pub async fn run() -> Result<(), Box<dyn Error>> {
    const ICON: &[u8] = include_bytes!("../../assets/icon.png");

    let initial_config = get_gui_config();

    let options = NativeOptions {
        vsync: true,
        centered: true,
        hardware_acceleration: HardwareAcceleration::Preferred,

        viewport: ViewportBuilder::default()
            .with_app_id("ru.arabianq.pwsp")
            .with_inner_size(Vec2::new(
                initial_config.window_width,
                initial_config.window_height,
            ))
            .with_min_inner_size(Vec2::new(800.0, 600.0))
            .with_icon(from_png_bytes(ICON)?),

        ..Default::default()
    };

    match run_native(
        "Pipewire Soundpad",
        options,
        Box::new(|cc| {
            egui_material_icons::initialize(&cc.egui_ctx);
            Ok(Box::new(SoundpadGui::new(&cc.egui_ctx)))
        }),
    ) {
        Ok(_) => {
            let config = get_gui_config();
            if config.pause_on_exit {
                make_request_sync(Request::pause(None)).ok();
            }
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}

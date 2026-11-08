pub mod args;
pub mod engine;
pub mod folder;
pub mod probe;
pub mod progress;
pub mod tools;

use engine::{Engine, Event, Job, JobRequest};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;
use tauri::{Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub folder: String,
    pub mode: args::Mode,
    pub max_height: Option<u32>,
    pub container: String,
    pub audio_format: String,
    pub audio_bitrate: Option<u32>,
    pub concurrency: usize,
    pub playlist_subfolder: bool,
    pub number_files: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            folder: String::new(),
            mode: args::Mode::Video,
            max_height: Some(1080),
            container: "mp4".into(),
            audio_format: "mp3".into(),
            audio_bitrate: None,
            concurrency: 3,
            playlist_subfolder: true,
            number_files: true,
        }
    }
}

struct AppState {
    engine: Engine,
    settings: Mutex<Settings>,
    settings_path: PathBuf,
    tools_dir: PathBuf,
}

/// Runs yt-dlp once to read a link's details.
pub fn run_probe(tools_dir: &Path, url: &str, no_playlist: bool) -> Result<probe::ProbeResult, String> {
    args::validate_url(url)?;
    let t = tools::resolve(tools_dir);
    let ytdlp = t.ytdlp.clone().ok_or("yt-dlp isn't installed yet. Install it from Components.")?;
    let out = tools::hidden_command(&ytdlp)
        .args(args::probe_args(url, no_playlist, &t))
        .env("PYTHONUTF8", "1")
        .env("PYTHONIOENCODING", "utf-8")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("Couldn't start yt-dlp: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(progress::friendly_error(&err, out.status.code()));
    }
    probe::parse(&String::from_utf8_lossy(&out.stdout), url)
}

#[derive(Serialize)]
struct Bootstrap {
    settings: Settings,
    jobs: Vec<Job>,
    version: &'static str,
}

#[tauri::command]
fn bootstrap(state: State<'_, AppState>) -> Bootstrap {
    Bootstrap {
        settings: state.settings.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        jobs: state.engine.jobs(),
        version: env!("CARGO_PKG_VERSION"),
    }
}

/// A fixed address, so the page can't ask this to open anything else.
#[tauri::command]
fn open_project_page(app: tauri::AppHandle) -> Result<(), String> {
    app.opener()
        .open_url("https://github.com/ra7bi/MediaCraft", None::<&str>)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn save_settings(state: State<'_, AppState>, settings: Settings) -> Result<(), String> {
    state.engine.set_concurrency(settings.concurrency);
    let text = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
    std::fs::write(&state.settings_path, text).map_err(|e| format!("Couldn't save settings: {e}"))?;
    *state.settings.lock().unwrap_or_else(|e| e.into_inner()) = settings;
    Ok(())
}

#[tauri::command(async)]
fn tools_status(state: State<'_, AppState>) -> Vec<tools::ToolInfo> {
    tools::status(&state.tools_dir)
}

#[derive(Clone, Serialize)]
struct ToolProgress {
    id: String,
    downloaded: u64,
    total: Option<u64>,
}

#[tauri::command(async)]
fn install_tool(app: tauri::AppHandle, state: State<'_, AppState>, id: String) -> Result<(), String> {
    let mut last = std::time::Instant::now() - std::time::Duration::from_secs(1);
    let last_cell = std::cell::RefCell::new(&mut last);
    let event_id = id.clone();
    tools::install(&id, &state.tools_dir, &|downloaded, total| {
        let mut l = last_cell.borrow_mut();
        if l.elapsed().as_millis() >= 150 {
            **l = std::time::Instant::now();
            let _ = app.emit("tool-progress", ToolProgress { id: event_id.clone(), downloaded, total });
        }
    })
}

#[tauri::command(async)]
fn probe_url(state: State<'_, AppState>, url: String, no_playlist: bool) -> Result<probe::ProbeResult, String> {
    run_probe(&state.tools_dir, &url, no_playlist)
}

#[tauri::command(async)]
fn check_folder(path: String) -> folder::FolderCheck {
    folder::check(Path::new(&path), false)
}

#[tauri::command(async)]
fn pick_folder(app: tauri::AppHandle) -> Option<String> {
    app.dialog().file().blocking_pick_folder().map(|p| p.to_string())
}

#[tauri::command(async)]
fn enqueue(state: State<'_, AppState>, requests: Vec<JobRequest>) -> Result<Vec<u64>, String> {
    state.engine.enqueue(requests)
}

#[tauri::command(async)]
fn cancel_job(state: State<'_, AppState>, id: u64) {
    state.engine.cancel(id)
}

#[tauri::command(async)]
fn cancel_all(state: State<'_, AppState>) {
    state.engine.cancel_all()
}

#[tauri::command(async)]
fn pause_job(state: State<'_, AppState>, id: u64) {
    state.engine.pause(id)
}

#[tauri::command(async)]
fn pause_all(state: State<'_, AppState>) {
    state.engine.pause_all()
}

#[tauri::command(async)]
fn resume_all(state: State<'_, AppState>) -> Result<(), String> {
    state.engine.resume_all()
}

#[tauri::command(async)]
fn retry_job(state: State<'_, AppState>, id: u64) -> Result<(), String> {
    state.engine.retry(id)
}

#[tauri::command]
fn remove_job(state: State<'_, AppState>, id: u64) {
    state.engine.remove(id)
}

#[tauri::command]
fn clear_finished(state: State<'_, AppState>) {
    state.engine.clear_finished()
}

/// Only opens a path belonging to a job in the queue or its folder.
#[tauri::command]
fn open_job(app: tauri::AppHandle, state: State<'_, AppState>, id: u64, reveal: bool) -> Result<(), String> {
    let job = state.engine.jobs().into_iter().find(|j| j.id == id).ok_or("That download is gone.")?;
    let opener = app.opener();
    match (&job.file, reveal) {
        (Some(f), _) if Path::new(f).exists() => {
            if reveal {
                opener.reveal_item_in_dir(f).map_err(|e| e.to_string())
            } else {
                opener.open_path(f.clone(), None::<&str>).map_err(|e| e.to_string())
            }
        }
        _ => {
            let dir = job.spec.target_dir();
            if !dir.exists() {
                return Err("That folder no longer exists.".into());
            }
            opener.open_path(dir.display().to_string(), None::<&str>).map_err(|e| e.to_string())
        }
    }
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let data = app.path().app_local_data_dir()?;
            std::fs::create_dir_all(&data)?;
            let tools_dir = tools::effective_tools_dir(&data.join("tools"));
            let settings_path = data.join("settings.json");

            let mut settings: Settings = std::fs::read_to_string(&settings_path)
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok())
                .unwrap_or_default();
            if settings.folder.is_empty() {
                settings.folder = app
                    .path()
                    .download_dir()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
            }

            let handle = app.handle().clone();
            let engine = Engine::new(
                tools_dir.clone(),
                Some(data.join("jobs.json")),
                settings.concurrency,
                move |ev| {
                    let _ = match ev {
                        Event::Updated(j) => handle.emit("job-updated", *j),
                        Event::Removed(id) => handle.emit("job-removed", id),
                    };
                },
            );

            app.manage(AppState { engine, settings: Mutex::new(settings), settings_path, tools_dir });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Destroyed = event {
                window.state::<AppState>().engine.cancel_all_running();
            }
        })
        .invoke_handler(tauri::generate_handler![
            bootstrap,
            save_settings,
            tools_status,
            install_tool,
            probe_url,
            check_folder,
            pick_folder,
            enqueue,
            cancel_job,
            cancel_all,
            pause_job,
            pause_all,
            resume_all,
            retry_job,
            remove_job,
            clear_finished,
            open_job,
            open_project_page
        ])
        .run(tauri::generate_context!())
        .expect("MediaCraft failed to start");
}

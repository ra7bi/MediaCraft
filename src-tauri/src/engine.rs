//! The download queue. Independent of the interface, so it can be tested directly.

use crate::args::{self, DownloadSpec};
use crate::folder;
use crate::progress::{self, Line};
use crate::tools::{self, ToolPaths};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Queued,
    Running,
    Paused,
    Done,
    Failed,
    Canceled,
    Interrupted,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobRequest {
    pub url: String,
    pub title: String,
    pub thumbnail: Option<String>,
    pub playlist: Option<String>,
    pub spec: DownloadSpec,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Job {
    pub id: u64,
    pub url: String,
    pub title: String,
    pub thumbnail: Option<String>,
    pub playlist: Option<String>,
    pub spec: DownloadSpec,
    pub status: Status,
    pub phase: String,
    pub fraction: Option<f64>,
    pub downloaded: Option<f64>,
    pub total: Option<f64>,
    pub speed: Option<f64>,
    pub eta: Option<f64>,
    pub file: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Debug)]
pub enum Event {
    Updated(Box<Job>),
    Removed(u64),
}

#[derive(Default)]
struct RunHandle {
    pid: Option<u32>,
    /// Set when the user stops the job: the status it should end in.
    stop: Option<Status>,
}

struct State {
    jobs: Vec<Job>,
    next_id: u64,
    concurrency: usize,
    running: HashMap<u64, RunHandle>,
}

struct Inner {
    state: Mutex<State>,
    emit: Box<dyn Fn(Event) + Send + Sync>,
    tools_dir: PathBuf,
    store: Option<PathBuf>,
}

#[derive(Clone)]
pub struct Engine {
    inner: Arc<Inner>,
}

fn lock(inner: &Inner) -> MutexGuard<'_, State> {
    inner.state.lock().unwrap_or_else(|e| e.into_inner())
}

const MIN_EVENT_GAP: Duration = Duration::from_millis(200);

impl Engine {
    pub fn new(
        tools_dir: PathBuf,
        store: Option<PathBuf>,
        concurrency: usize,
        emit: impl Fn(Event) + Send + Sync + 'static,
    ) -> Self {
        let jobs = store.as_deref().map(load_jobs).unwrap_or_default();
        let next_id = jobs.iter().map(|j| j.id).max().unwrap_or(0) + 1;
        Engine {
            inner: Arc::new(Inner {
                state: Mutex::new(State {
                    jobs,
                    next_id,
                    concurrency: concurrency.clamp(1, 8),
                    running: HashMap::new(),
                }),
                emit: Box::new(emit),
                tools_dir,
                store,
            }),
        }
    }

    pub fn jobs(&self) -> Vec<Job> {
        lock(&self.inner).jobs.clone()
    }

    pub fn set_concurrency(&self, n: usize) {
        lock(&self.inner).concurrency = n.clamp(1, 8);
        pump(&self.inner);
    }

    /// Everything is checked before a job exists, so a bad folder or missing
    /// tool is reported immediately rather than after analysing the video.
    pub fn enqueue(&self, requests: Vec<JobRequest>) -> Result<Vec<u64>, String> {
        let tools = tools::resolve(&self.inner.tools_dir);
        preflight_tools(&tools)?;
        let mut dirs: Vec<PathBuf> = vec![];
        for r in &requests {
            args::validate_url(&r.url)?;
            args::validate_spec(&r.spec)?;
            let d = r.spec.target_dir();
            if !dirs.contains(&d) {
                dirs.push(d);
            }
        }
        for d in &dirs {
            let c = folder::check(d, true);
            if !c.ok {
                return Err(c.message);
            }
        }
        let mut ids = vec![];
        let mut created = vec![];
        {
            let mut s = lock(&self.inner);
            for r in requests {
                let id = s.next_id;
                s.next_id += 1;
                let job = Job {
                    id,
                    url: r.url.trim().to_string(),
                    title: r.title,
                    thumbnail: r.thumbnail,
                    playlist: r.playlist,
                    spec: r.spec,
                    status: Status::Queued,
                    phase: "Waiting".into(),
                    fraction: None,
                    downloaded: None,
                    total: None,
                    speed: None,
                    eta: None,
                    file: None,
                    error: None,
                };
                created.push(job.clone());
                s.jobs.push(job);
                ids.push(id);
            }
        }
        for j in created {
            (self.inner.emit)(Event::Updated(Box::new(j)));
        }
        persist(&self.inner);
        pump(&self.inner);
        Ok(ids)
    }

    pub fn cancel(&self, id: u64) {
        self.halt(Status::Canceled, Some(id));
    }

    pub fn cancel_all(&self) {
        self.halt(Status::Canceled, None);
    }

    pub fn pause(&self, id: u64) {
        self.halt(Status::Paused, Some(id));
    }

    pub fn pause_all(&self) {
        self.halt(Status::Paused, None);
    }

    /// Stops one job or all of them. A running job's process is ended and its
    /// partial files stay on disk, which is what lets it continue later. Waiting
    /// jobs change under the same lock, so none can start while this happens.
    fn halt(&self, target: Status, only: Option<u64>) {
        let mut pids = vec![];
        let mut updated = vec![];
        {
            let mut guard = lock(&self.inner);
            let s = &mut *guard;
            for j in s.jobs.iter_mut().filter(|j| only.is_none_or(|id| id == j.id)) {
                if let Some(h) = s.running.get_mut(&j.id) {
                    h.stop = Some(target);
                    pids.extend(h.pid);
                } else if j.status == Status::Queued || (j.status == Status::Paused && target == Status::Canceled) {
                    j.status = target;
                    j.phase = stop_label(target).into();
                    updated.push(j.clone());
                }
            }
        }
        std::thread::scope(|t| {
            for pid in &pids {
                t.spawn(move || kill_tree(*pid));
            }
        });
        if !updated.is_empty() {
            for j in updated {
                (self.inner.emit)(Event::Updated(Box::new(j)));
            }
            persist(&self.inner);
        }
    }

    /// Continues everything paused, plus anything cut off when the app closed.
    /// One job that can't continue doesn't hold back the others.
    pub fn resume_all(&self) -> Result<(), String> {
        let ids: Vec<u64> = lock(&self.inner)
            .jobs
            .iter()
            .filter(|j| matches!(j.status, Status::Paused | Status::Interrupted))
            .map(|j| j.id)
            .collect();
        let mut first_error = None;
        for id in ids {
            if let Err(e) = self.retry(id) {
                first_error.get_or_insert(e);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    /// Partial files are kept, so this continues where the download stopped.
    /// It's also how a paused download resumes.
    pub fn retry(&self, id: u64) -> Result<(), String> {
        let job = {
            let s = lock(&self.inner);
            s.jobs.iter().find(|j| j.id == id).cloned()
        };
        let Some(job) = job else { return Ok(()) };
        if !matches!(job.status, Status::Paused | Status::Failed | Status::Canceled | Status::Interrupted) {
            return Ok(());
        }
        let c = folder::check(&job.spec.target_dir(), true);
        if !c.ok {
            return Err(c.message);
        }
        let tools = tools::resolve(&self.inner.tools_dir);
        preflight_tools(&tools)?;
        let updated = {
            let mut s = lock(&self.inner);
            s.jobs.iter_mut().find(|j| j.id == id).map(|j| {
                j.status = Status::Queued;
                j.phase = "Waiting".into();
                j.error = None;
                j.fraction = None;
                j.speed = None;
                j.eta = None;
                j.clone()
            })
        };
        if let Some(j) = updated {
            (self.inner.emit)(Event::Updated(Box::new(j)));
        }
        persist(&self.inner);
        pump(&self.inner);
        Ok(())
    }

    pub fn remove(&self, id: u64) {
        let removed = {
            let mut s = lock(&self.inner);
            let active = s.running.contains_key(&id);
            let before = s.jobs.len();
            if !active {
                s.jobs.retain(|j| j.id != id);
            }
            s.jobs.len() != before
        };
        if removed {
            (self.inner.emit)(Event::Removed(id));
            persist(&self.inner);
        }
    }

    pub fn clear_finished(&self) {
        let removed: Vec<u64> = {
            let mut s = lock(&self.inner);
            let gone = s.jobs.iter().filter(|j| j.status == Status::Done).map(|j| j.id).collect();
            s.jobs.retain(|j| j.status != Status::Done);
            gone
        };
        for id in removed {
            (self.inner.emit)(Event::Removed(id));
        }
        persist(&self.inner);
    }

    pub fn cancel_all_running(&self) {
        let pids: Vec<u32> = {
            let mut s = lock(&self.inner);
            s.running
                .values_mut()
                .filter_map(|h| {
                    h.stop = Some(Status::Canceled);
                    h.pid
                })
                .collect()
        };
        for p in pids {
            kill_tree(p);
        }
    }
}

fn stop_label(status: Status) -> &'static str {
    if status == Status::Paused { "Paused" } else { "Canceled" }
}

fn preflight_tools(tools: &ToolPaths) -> Result<(), String> {
    if tools.ytdlp.is_none() {
        return Err("yt-dlp isn't installed yet. Install it from Components.".into());
    }
    if tools.ffmpeg.is_none() {
        return Err("FFmpeg isn't installed yet. Install it from Components.".into());
    }
    Ok(())
}

fn load_jobs(path: &Path) -> Vec<Job> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return vec![];
    };
    let mut jobs: Vec<Job> = serde_json::from_str(&text).unwrap_or_default();
    for j in &mut jobs {
        if matches!(j.status, Status::Running | Status::Queued) {
            j.status = Status::Interrupted;
            j.phase = "Interrupted".into();
            j.speed = None;
            j.eta = None;
        }
    }
    jobs
}

fn persist(inner: &Inner) {
    let Some(path) = &inner.store else { return };
    let jobs = lock(inner).jobs.clone();
    if let Ok(text) = serde_json::to_string_pretty(&jobs) {
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }
}

fn kill_tree(pid: u32) {
    // yt-dlp.exe starts a child process and may start FFmpeg, so stop the whole tree.
    let _ = tools::hidden_command(Path::new("taskkill"))
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn pump(inner: &Arc<Inner>) {
    let mut to_start = vec![];
    {
        let mut s = lock(inner);
        loop {
            if s.running.len() >= s.concurrency {
                break;
            }
            let Some(j) = s.jobs.iter_mut().find(|j| j.status == Status::Queued) else {
                break;
            };
            j.status = Status::Running;
            j.phase = "Preparing".into();
            let (id, snapshot) = (j.id, j.clone());
            s.running.insert(id, RunHandle::default());
            to_start.push((id, snapshot));
        }
    }
    for (id, snapshot) in to_start {
        (inner.emit)(Event::Updated(Box::new(snapshot)));
        let inner = inner.clone();
        std::thread::spawn(move || run_job(inner, id));
    }
}

fn update(inner: &Inner, id: u64, f: impl FnOnce(&mut Job)) -> Option<Job> {
    let mut s = lock(inner);
    s.jobs.iter_mut().find(|j| j.id == id).map(|j| {
        f(j);
        j.clone()
    })
}

fn update_emit(inner: &Inner, id: u64, f: impl FnOnce(&mut Job)) {
    if let Some(j) = update(inner, id, f) {
        (inner.emit)(Event::Updated(Box::new(j)));
    }
}

fn finish(inner: &Arc<Inner>, id: u64, f: impl FnOnce(&mut Job)) {
    update_emit(inner, id, |j| {
        f(j);
        j.speed = None;
        j.eta = None;
    });
    lock(inner).running.remove(&id);
    persist(inner);
    pump(inner);
}

fn run_job(inner: Arc<Inner>, id: u64) {
    let Some(job) = lock(&inner).jobs.iter().find(|j| j.id == id).cloned() else {
        lock(&inner).running.remove(&id);
        return;
    };

    let check = folder::check(&job.spec.target_dir(), true);
    if !check.ok {
        return finish(&inner, id, |j| {
            j.status = Status::Failed;
            j.phase = "Failed".into();
            j.error = Some(check.message);
        });
    }

    let tools = tools::resolve(&inner.tools_dir);
    let Some(ytdlp) = tools.ytdlp.clone() else {
        return finish(&inner, id, |j| {
            j.status = Status::Failed;
            j.phase = "Failed".into();
            j.error = Some("yt-dlp isn't installed yet. Install it from Components.".into());
        });
    };

    let spawned = tools::hidden_command(&ytdlp)
        .args(args::download_args(&job.spec, &job.url, &tools))
        .env("PYTHONUTF8", "1")
        .env("PYTHONIOENCODING", "utf-8")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();

    let mut child = match spawned {
        Ok(c) => c,
        Err(e) => {
            return finish(&inner, id, |j| {
                j.status = Status::Failed;
                j.phase = "Failed".into();
                j.error = Some(format!("Couldn't start yt-dlp: {e}"));
            })
        }
    };

    let cancel_now = {
        let mut s = lock(&inner);
        match s.running.get_mut(&id) {
            Some(h) => {
                h.pid = Some(child.id());
                h.stop.is_some()
            }
            None => false,
        }
    };
    if cancel_now {
        kill_tree(child.id());
    }

    let stderr = child.stderr.take();
    let stderr_reader = std::thread::spawn(move || {
        let mut tail: Vec<String> = vec![];
        if let Some(e) = stderr {
            for line in BufReader::new(e).split(b'\n').map_while(Result::ok) {
                tail.push(String::from_utf8_lossy(&line).trim_end().to_string());
                if tail.len() > 60 {
                    tail.remove(0);
                }
            }
        }
        tail.join("\n")
    });

    let mut file: Option<String> = None;
    if let Some(out) = child.stdout.take() {
        let mut last_emit = Instant::now() - MIN_EVENT_GAP;
        let mut streams_seen: HashSet<bool> = HashSet::new();
        for raw in BufReader::new(out).split(b'\n').map_while(Result::ok) {
            let line = String::from_utf8_lossy(&raw);
            match progress::parse_line(&line) {
                Line::Download { status, downloaded, total, speed, eta, is_audio } => {
                    streams_seen.insert(is_audio);
                    let phase = if is_audio { "Downloading audio" } else { "Downloading video" };
                    let finished = status == "finished";
                    let changed = |j: &mut Job| {
                        j.phase = phase.to_string();
                        j.downloaded = downloaded;
                        j.total = total;
                        j.speed = speed;
                        j.eta = eta;
                        j.fraction = match (downloaded, total) {
                            (Some(d), Some(t)) if t > 0.0 => Some((d / t).min(1.0)),
                            _ if finished => Some(1.0),
                            _ => None,
                        };
                    };
                    if finished || last_emit.elapsed() >= MIN_EVENT_GAP {
                        update_emit(&inner, id, changed);
                        last_emit = Instant::now();
                    } else {
                        update(&inner, id, changed);
                    }
                }
                Line::PostProcess { status, name } => {
                    if status == "started" {
                        let label = progress::postprocess_label(&name);
                        update_emit(&inner, id, |j| {
                            j.phase = label;
                            j.fraction = None;
                            j.speed = None;
                            j.eta = None;
                        });
                    }
                }
                Line::File(p) => file = Some(p),
                Line::Other => {}
            }
        }
    }

    let exit = child.wait();
    let err_text = stderr_reader.join().unwrap_or_default();
    let stop = lock(&inner).running.get(&id).and_then(|h| h.stop);

    finish(&inner, id, |j| {
        match exit {
            // A stop that arrives just as the download completes doesn't undo it.
            Ok(code) if code.success() => {
                j.status = Status::Done;
                j.phase = "Done".into();
                j.fraction = Some(1.0);
                // Report the finished file, not the last stream that was downloaded.
                let size = file.as_deref().and_then(|f| std::fs::metadata(f).ok()).map(|m| m.len() as f64);
                j.total = size;
                j.downloaded = size;
                j.file = file;
                j.error = None;
            }
            _ if stop.is_some() => {
                let target = stop.unwrap_or(Status::Canceled);
                j.status = target;
                j.phase = stop_label(target).into();
            }
            Ok(code) => {
                j.status = Status::Failed;
                j.phase = "Failed".into();
                j.error = Some(progress::friendly_error(&err_text, code.code()));
            }
            Err(e) => {
                j.status = Status::Failed;
                j.phase = "Failed".into();
                j.error = Some(format!("Lost track of yt-dlp: {e}"));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::Mode;

    fn job(id: u64, status: Status) -> Job {
        Job {
            id,
            url: "https://x/v".into(),
            title: "t".into(),
            thumbnail: None,
            playlist: None,
            spec: DownloadSpec {
                mode: Mode::Video,
                max_height: None,
                container: "mp4".into(),
                audio_format: "mp3".into(),
                audio_bitrate: None,
                folder: "C:\\x".into(),
                subfolder: None,
                index: None,
            },
            status,
            phase: String::new(),
            fraction: None,
            downloaded: None,
            total: None,
            speed: None,
            eta: None,
            file: None,
            error: None,
        }
    }

    #[test]
    fn unfinished_jobs_become_interrupted_on_restart_and_ids_continue() {
        let p = std::env::temp_dir().join(format!("mediacraft-jobs-{}.json", std::process::id()));
        let saved = vec![job(3, Status::Running), job(5, Status::Queued), job(9, Status::Done)];
        std::fs::write(&p, serde_json::to_string(&saved).unwrap()).unwrap();
        let e = Engine::new(PathBuf::from("C:\\none"), Some(p.clone()), 2, |_| {});
        let j = e.jobs();
        assert_eq!(j[0].status, Status::Interrupted);
        assert_eq!(j[1].status, Status::Interrupted);
        assert_eq!(j[2].status, Status::Done);
        assert_eq!(lock(&e.inner).next_id, 10);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn corrupt_store_starts_empty() {
        let p = std::env::temp_dir().join(format!("mediacraft-bad-{}.json", std::process::id()));
        std::fs::write(&p, "{oops").unwrap();
        let e = Engine::new(PathBuf::from("C:\\none"), Some(p.clone()), 2, |_| {});
        assert!(e.jobs().is_empty());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn concurrency_is_clamped() {
        let e = Engine::new(PathBuf::from("C:\\none"), None, 99, |_| {});
        assert_eq!(lock(&e.inner).concurrency, 8);
        e.set_concurrency(0);
        assert_eq!(lock(&e.inner).concurrency, 1);
    }

    #[test]
    fn queued_job_cancels_without_a_process_and_can_be_removed() {
        let e = Engine::new(PathBuf::from("C:\\none"), None, 1, |_| {});
        lock(&e.inner).jobs.push(job(1, Status::Queued));
        e.cancel(1);
        assert_eq!(e.jobs()[0].status, Status::Canceled);
        e.remove(1);
        assert!(e.jobs().is_empty());
    }

    #[test]
    fn pause_all_holds_waiting_jobs_and_flags_running_ones() {
        let e = Engine::new(PathBuf::from("C:\\none"), None, 1, |_| {});
        {
            let mut s = lock(&e.inner);
            s.jobs.push(job(1, Status::Running));
            s.running.insert(1, RunHandle::default());
            s.jobs.push(job(2, Status::Queued));
            s.jobs.push(job(3, Status::Done));
            s.jobs.push(job(4, Status::Failed));
        }
        e.pause_all();
        assert_eq!(lock(&e.inner).running[&1].stop, Some(Status::Paused));
        let j = e.jobs();
        assert_eq!(j[1].status, Status::Paused);
        assert_eq!(j[2].status, Status::Done);
        assert_eq!(j[3].status, Status::Failed);
        // Nothing waiting is left for a freed slot to pick up.
        lock(&e.inner).running.remove(&1);
        pump(&e.inner);
        assert!(lock(&e.inner).running.is_empty());
    }

    #[test]
    fn pause_only_touches_the_chosen_job() {
        let e = Engine::new(PathBuf::from("C:\\none"), None, 1, |_| {});
        {
            let mut s = lock(&e.inner);
            s.jobs.push(job(1, Status::Queued));
            s.jobs.push(job(2, Status::Queued));
        }
        e.pause(2);
        let j = e.jobs();
        assert_eq!(j[0].status, Status::Queued);
        assert_eq!(j[1].status, Status::Paused);
    }

    #[test]
    fn cancel_all_covers_running_waiting_and_paused_only() {
        let e = Engine::new(PathBuf::from("C:\\none"), None, 1, |_| {});
        {
            let mut s = lock(&e.inner);
            s.jobs.push(job(1, Status::Running));
            s.running.insert(1, RunHandle::default());
            s.jobs.push(job(2, Status::Queued));
            s.jobs.push(job(3, Status::Paused));
            s.jobs.push(job(4, Status::Done));
            s.jobs.push(job(5, Status::Interrupted));
        }
        e.cancel_all();
        assert_eq!(lock(&e.inner).running[&1].stop, Some(Status::Canceled));
        let j = e.jobs();
        assert_eq!(j[1].status, Status::Canceled);
        assert_eq!(j[2].status, Status::Canceled);
        assert_eq!(j[3].status, Status::Done);
        assert_eq!(j[4].status, Status::Interrupted);
    }

    #[test]
    fn pausing_a_paused_job_changes_nothing() {
        let e = Engine::new(PathBuf::from("C:\\none"), None, 1, |_| {});
        lock(&e.inner).jobs.push(job(1, Status::Paused));
        e.pause(1);
        assert_eq!(e.jobs()[0].status, Status::Paused);
    }

    #[test]
    fn paused_jobs_stay_paused_after_a_restart() {
        let p = std::env::temp_dir().join(format!("mediacraft-paused-{}.json", std::process::id()));
        std::fs::write(&p, serde_json::to_string(&vec![job(1, Status::Paused)]).unwrap()).unwrap();
        let e = Engine::new(PathBuf::from("C:\\none"), Some(p.clone()), 2, |_| {});
        assert_eq!(e.jobs()[0].status, Status::Paused);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn clear_finished_keeps_everything_else() {
        let e = Engine::new(PathBuf::from("C:\\none"), None, 1, |_| {});
        {
            let mut s = lock(&e.inner);
            s.jobs.push(job(1, Status::Done));
            s.jobs.push(job(2, Status::Failed));
        }
        e.clear_finished();
        assert_eq!(e.jobs().len(), 1);
        assert_eq!(e.jobs()[0].id, 2);
    }
}

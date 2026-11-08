//! Real downloads through the real engine. They need the network and tools, so
//! they are ignored by default. Run with:
//!   set MEDIACRAFT_TOOLS_DIR=<folder holding yt-dlp.exe>
//!   cargo test --test e2e -- --ignored --test-threads=1 --nocapture
//! Every output is checked with ffprobe, not just by exit code.

use mediacraft_lib::args::{DownloadSpec, Mode};
use mediacraft_lib::engine::{Engine, Event, Job, JobRequest, Status};
use mediacraft_lib::{run_probe, tools};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const ZOO: &str = "https://www.youtube.com/watch?v=jNQXAC9IVRw";
/// "PSY - GANGNAM STYLE(강남스타일) M/V": Korean text and a slash in the title.
const ODD_TITLE: &str = "https://www.youtube.com/watch?v=9bZkp7q19f0";
const LONG_VIDEO: &str = "https://www.youtube.com/watch?v=aqz-KE-bpKQ";
const PLAYLIST: &str = "https://www.youtube.com/playlist?list=PLBCF2DAC6FFB574DE";

fn tools_dir() -> PathBuf {
    std::env::var_os("MEDIACRAFT_TOOLS_DIR").map(PathBuf::from).expect("set MEDIACRAFT_TOOLS_DIR")
}

fn out_dir(name: &str) -> PathBuf {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("test-output").join(name);
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    std::fs::canonicalize(&p).unwrap()
}

fn plain(p: &Path) -> String {
    p.display().to_string().trim_start_matches(r"\\?\").to_string()
}

fn video(folder: &Path, h: Option<u32>, c: &str) -> DownloadSpec {
    DownloadSpec {
        mode: Mode::Video,
        max_height: h,
        container: c.into(),
        audio_format: "mp3".into(),
        audio_bitrate: None,
        folder: plain(folder),
        subfolder: None,
        index: None,
    }
}

fn audio(folder: &Path, f: &str, b: Option<u32>) -> DownloadSpec {
    DownloadSpec { mode: Mode::Audio, audio_format: f.into(), audio_bitrate: b, ..video(folder, None, "mp4") }
}

fn req(url: &str, spec: DownloadSpec) -> JobRequest {
    JobRequest { url: url.into(), title: "test".into(), thumbnail: None, playlist: None, spec }
}

fn wait_all(e: &Engine, secs: u64) -> Vec<Job> {
    let t = Instant::now();
    loop {
        let jobs = e.jobs();
        if jobs.iter().all(|j| !matches!(j.status, Status::Queued | Status::Running)) {
            return jobs;
        }
        assert!(t.elapsed() < Duration::from_secs(secs), "timed out: {jobs:#?}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn engine() -> Engine {
    Engine::new(tools_dir(), None, 3, |_| {})
}

fn probe_file(path: &str) -> Value {
    let t = tools::resolve(&tools_dir());
    let ffprobe = t.ffmpeg.unwrap().with_file_name("ffprobe.exe");
    let out = tools::hidden_command(&ffprobe)
        .args(["-v", "error", "-show_streams", "-show_format", "-of", "json", path])
        .output()
        .unwrap();
    assert!(out.status.success(), "ffprobe failed on {path}: {}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice(&out.stdout).unwrap()
}

fn streams(v: &Value, kind: &str) -> Vec<Value> {
    v["streams"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["codec_type"] == kind && s["disposition"]["attached_pic"] != 1)
        .cloned()
        .collect()
}

fn done_file(j: &Job) -> String {
    assert_eq!(j.status, Status::Done, "job failed: {:?}", j.error);
    let f = j.file.clone().unwrap_or_else(|| panic!("no file reported: {j:#?}"));
    assert!(Path::new(&f).is_file(), "reported file missing: {f}");
    let real = std::fs::metadata(&f).unwrap().len() as f64;
    assert_eq!(j.total, Some(real), "displayed size must be the real file size");
    f
}

fn dir_size(d: &Path) -> u64 {
    std::fs::read_dir(d).unwrap().map(|e| e.unwrap().metadata().unwrap().len()).sum()
}

/// Downloads and verifies each component into a temp folder. Doesn't run them.
#[test]
#[ignore]
fn component_installers_verify_and_unpack_without_running() {
    let base = std::env::temp_dir().join(format!("mediacraft-install-{}", std::process::id()));
    for (id, expect) in [("yt-dlp", "yt-dlp.exe"), ("ffmpeg", "ffmpeg.exe"), ("js", "deno.exe")] {
        let d = base.join(id);
        let _ = std::fs::remove_dir_all(&d);
        tools::install(id, &d, &|_, _| {}).unwrap_or_else(|e| panic!("{id}: {e}"));
        assert!(d.join(expect).is_file(), "{id}: {expect} missing");
        assert!(!d.join(format!("{id}.download")).exists(), "{id}: temp file left");
        println!("OK {id}: {} files, {:.1} MB on disk", std::fs::read_dir(&d).unwrap().count(), dir_size(&d) as f64 / 1048576.0);
    }
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
#[ignore]
fn single_video_each_container_and_height_cap() {
    let d = out_dir("containers");
    for (c, fmt) in [("mp4", "mp4"), ("mkv", "matroska"), ("webm", "matroska,webm")] {
        let e = engine();
        e.enqueue(vec![req(ZOO, video(&d.join(c), Some(240), c))]).unwrap();
        let f = done_file(&wait_all(&e, 180)[0]);
        assert!(f.ends_with(&format!(".{c}")), "{f}");
        let p = probe_file(&f);
        let names = p["format"]["format_name"].as_str().unwrap();
        assert!(names.contains(fmt.split(',').next().unwrap()), "{c}: {names}");
        let v = streams(&p, "video");
        assert_eq!(v.len(), 1, "{c}: needs one video stream");
        assert_eq!(streams(&p, "audio").len(), 1, "{c}: needs one audio stream");
        assert!(v[0]["height"].as_u64().unwrap() <= 240, "{c}: height cap ignored");
        println!("OK {c}: {names} {}x{}", v[0]["width"], v[0]["height"]);
    }
}

#[test]
#[ignore]
fn quality_choice_changes_the_result() {
    let d = out_dir("quality");
    let e = engine();
    e.enqueue(vec![
        req(LONG_VIDEO, video(&d.join("h144"), Some(144), "mp4")),
        req(LONG_VIDEO, video(&d.join("h360"), Some(360), "mp4")),
    ])
    .unwrap();
    let jobs = wait_all(&e, 600);
    let h = |j: &Job| streams(&probe_file(&done_file(j)), "video")[0]["height"].as_u64().unwrap();
    assert_eq!(h(&jobs[0]), 144);
    assert_eq!(h(&jobs[1]), 360);
    println!("OK heights 144 and 360");
}

#[test]
#[ignore]
fn audio_formats_have_no_video_stream() {
    let d = out_dir("audio");
    for (f, codec) in [("mp3", "mp3"), ("m4a", "aac"), ("opus", "opus"), ("flac", "flac"), ("wav", "pcm_s16le")] {
        let e = engine();
        e.enqueue(vec![req(ZOO, audio(&d, f, None))]).unwrap();
        let path = done_file(&wait_all(&e, 180)[0]);
        assert!(path.ends_with(&format!(".{f}")), "{path}");
        let p = probe_file(&path);
        assert!(streams(&p, "video").is_empty(), "{f}: has video");
        let a = streams(&p, "audio");
        assert_eq!(a[0]["codec_name"], codec, "{f}");
        println!("OK {f}: {}", a[0]["codec_name"]);
    }
}

#[test]
#[ignore]
fn audio_bitrate_choice_is_respected() {
    let d = out_dir("bitrate");
    let e = engine();
    e.enqueue(vec![req(ZOO, audio(&d, "mp3", Some(128)))]).unwrap();
    let p = probe_file(&done_file(&wait_all(&e, 180)[0]));
    let b: u64 = streams(&p, "audio")[0]["bit_rate"].as_str().unwrap().parse().unwrap();
    assert!((110_000..=140_000).contains(&b), "bitrate {b}");
    println!("OK mp3 bitrate {b}");
}

#[test]
#[ignore]
fn awkward_title_downloads_and_is_reported_correctly() {
    let d = out_dir("odd title");
    let e = engine();
    e.enqueue(vec![req(ODD_TITLE, video(&d, Some(360), "mp4"))]).unwrap();
    let f = done_file(&wait_all(&e, 180)[0]);
    println!("OK file: {f}");
    assert!(!f.contains('�'), "encoding damaged: {f}");
    probe_file(&f);
}

#[test]
#[ignore]
fn probe_reads_a_video_and_a_playlist() {
    let v = run_probe(&tools_dir(), ZOO, false).unwrap();
    assert_eq!(v.kind, "video");
    assert_eq!(v.title, "Me at the zoo");
    assert!(!v.heights.is_empty());
    let p = run_probe(&tools_dir(), PLAYLIST, false).unwrap();
    assert_eq!(p.kind, "playlist");
    assert!(p.entries.len() >= 2, "{p:#?}");
    println!("OK video heights {:?}; playlist '{}' with {} entries", v.heights.iter().map(|h| h.height).collect::<Vec<_>>(), p.title, p.entries.len());
}

#[test]
#[ignore]
fn complete_playlist_goes_to_named_folder_with_numbers_and_respects_limit() {
    let d = out_dir("playlist");
    let p = run_probe(&tools_dir(), PLAYLIST, false).unwrap();

    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let state: Arc<Mutex<std::collections::HashMap<u64, bool>>> = Default::default();
    let (a2, p2, s2) = (active.clone(), peak.clone(), state.clone());
    let e = Engine::new(tools_dir(), None, 2, move |ev| {
        if let Event::Updated(j) = ev {
            let mut m = s2.lock().unwrap();
            let now = j.status == Status::Running;
            let was = m.insert(j.id, now).unwrap_or(false);
            if now && !was {
                let n = a2.fetch_add(1, Ordering::SeqCst) + 1;
                p2.fetch_max(n, Ordering::SeqCst);
            } else if !now && was {
                a2.fetch_sub(1, Ordering::SeqCst);
            }
        }
    });
    // Playlists can hold removed videos, so take four that still have details.
    let requests: Vec<JobRequest> = p
        .entries
        .iter()
        .enumerate()
        .filter(|(_, en)| en.duration.is_some())
        .take(4)
        .map(|(i, en)| {
            let mut s = video(&d, Some(360), "mp4");
            s.subfolder = Some(p.title.clone());
            s.index = Some(i as u32 + 1);
            JobRequest { url: en.url.clone(), title: en.title.clone(), thumbnail: None, playlist: Some(p.title.clone()), spec: s }
        })
        .collect();
    let n = requests.len();
    e.enqueue(requests).unwrap();
    let jobs = wait_all(&e, 600);
    assert_eq!(jobs.len(), n);
    let mut names: Vec<String> = jobs
        .iter()
        .map(|j| {
            let f = done_file(j);
            let path = PathBuf::from(&f);
            assert!(path.parent().unwrap().ends_with(mediacraft_lib::args::sanitize_component(&p.title)), "{f}");
            path.file_name().unwrap().to_string_lossy().to_string()
        })
        .collect();
    names.sort();
    assert!(names[0].starts_with("001 - "), "{names:?}");
    assert_eq!(names.iter().filter(|n| n.as_bytes()[3] == b' ').count(), n, "{names:?}");
    assert!(peak.load(Ordering::SeqCst) <= 2, "ran {} at once", peak.load(Ordering::SeqCst));
    println!("OK {n} files, peak {} at once: {names:#?}", peak.load(Ordering::SeqCst));
}

#[test]
#[ignore]
fn cancel_stops_processes_keeps_partial_and_retry_resumes() {
    let d = out_dir("resume");
    let first_bytes = Arc::new(AtomicU32::new(0));
    let seen = first_bytes.clone();
    let progress_log: Arc<Mutex<Vec<f64>>> = Default::default();
    let log2 = progress_log.clone();
    let e = Engine::new(tools_dir(), None, 1, move |ev| {
        if let Event::Updated(j) = ev {
            if let Some(b) = j.downloaded.filter(|_| j.status == Status::Running) {
                seen.fetch_add(1, Ordering::SeqCst);
                log2.lock().unwrap().push(b);
            }
        }
    });
    let id = e.enqueue(vec![req(LONG_VIDEO, video(&d, Some(360), "mp4"))]).unwrap()[0];

    let t = Instant::now();
    loop {
        let got = e.jobs()[0].downloaded.unwrap_or(0.0);
        if got > 3_000_000.0 {
            break;
        }
        assert!(t.elapsed() < Duration::from_secs(120), "never made progress: {:#?}", e.jobs());
        let j = &e.jobs()[0];
        assert_eq!(j.status, Status::Running, "stopped early: error={:?} phase={} downloaded={:?}", j.error, j.phase, j.downloaded);
        std::thread::sleep(Duration::from_millis(50));
    }
    e.cancel(id);
    let jobs = wait_all(&e, 30);
    assert_eq!(jobs[0].status, Status::Canceled);
    let at_cancel = jobs[0].downloaded.unwrap();

    std::thread::sleep(Duration::from_millis(800));
    let list = tools::hidden_command(Path::new("tasklist"))
        .args(["/FI", "IMAGENAME eq yt-dlp.exe", "/NH"])
        .output()
        .unwrap();
    assert!(!String::from_utf8_lossy(&list.stdout).contains("yt-dlp.exe"), "yt-dlp still running after cancel");

    let parts: Vec<String> = std::fs::read_dir(&d)
        .unwrap()
        .map(|x| x.unwrap().file_name().to_string_lossy().to_string())
        .filter(|n| n.contains(".part"))
        .collect();
    assert!(!parts.is_empty(), "partial file should be kept");
    println!("canceled at {at_cancel} bytes, kept {parts:?}");

    progress_log.lock().unwrap().clear();
    e.retry(id).unwrap();
    let jobs = wait_all(&e, 600);
    let f = done_file(&jobs[0]);
    let first_after = progress_log.lock().unwrap().first().copied().unwrap();
    assert!(first_after >= at_cancel * 0.9, "restarted from {first_after}, canceled at {at_cancel}");
    assert_eq!(streams(&probe_file(&f), "video")[0]["height"], 360);
    let leftovers: Vec<_> = std::fs::read_dir(&d)
        .unwrap()
        .map(|x| x.unwrap().file_name().to_string_lossy().to_string())
        .filter(|n| n.contains(".part"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
    println!("OK resumed from {first_after} bytes (canceled at {at_cancel}); final {f}");
}

fn wait_for_bytes(e: &Engine, id: u64, min: f64) {
    let t = Instant::now();
    loop {
        let jobs = e.jobs();
        let j = jobs.iter().find(|j| j.id == id).unwrap();
        if j.downloaded.unwrap_or(0.0) > min {
            return;
        }
        assert!(t.elapsed() < Duration::from_secs(120), "never made progress: {j:#?}");
        assert_eq!(j.status, Status::Running, "stopped early: {:?}", j.error);
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn yt_dlp_running() -> bool {
    let list = tools::hidden_command(Path::new("tasklist"))
        .args(["/FI", "IMAGENAME eq yt-dlp.exe", "/NH"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&list.stdout).contains("yt-dlp.exe")
}

#[test]
#[ignore]
fn pause_all_stops_everything_and_resume_all_finishes_from_the_partial_files() {
    let d = out_dir("pause-all");
    let log: Arc<Mutex<Vec<(u64, f64)>>> = Default::default();
    let log2 = log.clone();
    let e = Engine::new(tools_dir(), None, 2, move |ev| {
        if let Event::Updated(j) = ev {
            if let Some(b) = j.downloaded.filter(|_| j.status == Status::Running) {
                log2.lock().unwrap().push((j.id, b));
            }
        }
    });
    // Two run and the third waits, so this also proves a freed slot isn't refilled.
    let ids = e
        .enqueue(vec![
            req(LONG_VIDEO, video(&d.join("a"), Some(360), "mp4")),
            req(LONG_VIDEO, video(&d.join("b"), Some(240), "mp4")),
            req(ZOO, video(&d.join("c"), Some(240), "mp4")),
        ])
        .unwrap();
    wait_for_bytes(&e, ids[0], 2_000_000.0);
    e.pause_all();
    let jobs = wait_all(&e, 30);
    assert!(jobs.iter().all(|j| j.status == Status::Paused), "{jobs:#?}");
    let at_pause = jobs[0].downloaded.unwrap();
    std::thread::sleep(Duration::from_secs(2));
    assert!(!yt_dlp_running(), "yt-dlp still running after pause all");
    assert!(e.jobs().iter().all(|j| j.status == Status::Paused), "something restarted");
    let c_empty = !d.join("c").exists() || std::fs::read_dir(d.join("c")).unwrap().count() == 0;
    assert!(c_empty, "the waiting job should not have started");

    log.lock().unwrap().clear();
    e.resume_all().unwrap();
    let jobs = wait_all(&e, 600);
    for j in &jobs {
        probe_file(&done_file(j));
    }
    let first = log.lock().unwrap().iter().find(|(id, _)| *id == ids[0]).unwrap().1;
    assert!(first >= at_pause * 0.9, "restarted from {first}, paused at {at_pause}");
    println!("OK paused at {at_pause} bytes, resumed from {first}; all three files verified");
}

#[test]
#[ignore]
fn cancel_all_stops_running_and_waiting_downloads() {
    let d = out_dir("cancel-all");
    let e = Engine::new(tools_dir(), None, 1, |_| {});
    let ids = e
        .enqueue(vec![
            req(LONG_VIDEO, video(&d.join("a"), Some(360), "mp4")),
            req(ZOO, video(&d.join("b"), Some(240), "mp4")),
        ])
        .unwrap();
    wait_for_bytes(&e, ids[0], 1_000_000.0);
    e.cancel_all();
    let jobs = wait_all(&e, 30);
    assert!(jobs.iter().all(|j| j.status == Status::Canceled), "{jobs:#?}");
    std::thread::sleep(Duration::from_secs(2));
    assert!(!yt_dlp_running(), "yt-dlp still running after cancel all");
    assert!(e.jobs().iter().all(|j| j.status == Status::Canceled), "something restarted");
    println!("OK both canceled, no yt-dlp left");
}

#[test]
#[ignore]
fn downloading_the_same_video_twice_still_succeeds() {
    let d = out_dir("twice");
    for _ in 0..2 {
        let e = engine();
        e.enqueue(vec![req(ZOO, video(&d, Some(240), "mp4"))]).unwrap();
        let j = wait_all(&e, 180).remove(0);
        println!("status {:?} file {:?}", j.status, j.file);
        done_file(&j);
    }
    println!("OK");
}

#[test]
#[ignore]
fn missing_drive_is_refused_before_any_work() {
    let drive = ('D'..='Z').map(|c| format!("{c}:\\")).find(|d| !Path::new(d).exists()).unwrap();
    let spec = video(&Path::new(&drive).join("Youtube").join("Courses"), None, "mp4");
    let t = Instant::now();
    let err = engine().enqueue(vec![req(ZOO, spec)]).unwrap_err();
    assert!(err.contains(&drive[..2]), "{err}");
    assert!(t.elapsed() < Duration::from_secs(1), "took {:?}", t.elapsed());
    println!("OK in {:?}: {err}", t.elapsed());
}

#[test]
#[ignore]
fn bad_links_give_clear_messages() {
    let dead = run_probe(&tools_dir(), "https://www.youtube.com/watch?v=aaaaaaaaaaa", false).unwrap_err();
    println!("dead video: {dead}");
    assert!(!dead.to_lowercase().contains("unknown error"));
    let unsupported = run_probe(&tools_dir(), "https://example.invalid/nothing", false).unwrap_err();
    println!("bad host: {unsupported}");
    assert!(!unsupported.is_empty());
    assert!(run_probe(&tools_dir(), "--exec calc", false).unwrap_err().contains("web link"));
}

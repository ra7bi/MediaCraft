//! Builds yt-dlp command lines. Pure functions, so every option is unit tested.

use crate::tools::ToolPaths;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Video,
    Audio,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DownloadSpec {
    pub mode: Mode,
    /// Maximum video height. `None` means the best available.
    pub max_height: Option<u32>,
    /// mp4, mkv or webm.
    pub container: String,
    /// mp3, m4a, opus, flac or wav.
    pub audio_format: String,
    /// Bitrate in kbit/s for lossy audio. `None` means the best available.
    pub audio_bitrate: Option<u32>,
    pub folder: String,
    pub subfolder: Option<String>,
    /// Numbers the file, for example "003 - Title", to keep playlist order.
    pub index: Option<u32>,
}

impl DownloadSpec {
    pub fn target_dir(&self) -> PathBuf {
        let mut p = PathBuf::from(&self.folder);
        if let Some(s) = &self.subfolder {
            p.push(sanitize_component(s));
        }
        p
    }
}

pub fn validate_spec(spec: &DownloadSpec) -> Result<(), String> {
    if !["mp4", "mkv", "webm"].contains(&spec.container.as_str()) {
        return Err(format!("Unknown video format: {}", spec.container));
    }
    if !["mp3", "m4a", "opus", "flac", "wav"].contains(&spec.audio_format.as_str()) {
        return Err(format!("Unknown audio format: {}", spec.audio_format));
    }
    Ok(())
}

pub fn validate_url(url: &str) -> Result<(), String> {
    let u = url.trim();
    let lower = u.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return Err("That doesn't look like a web link. It should start with https://".into());
    }
    if u.chars().any(char::is_whitespace) {
        return Err("A link can't contain spaces. Paste one link at a time.".into());
    }
    Ok(())
}

/// Makes a string safe as one Windows folder name.
pub fn sanitize_component(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' | '%' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim().trim_end_matches(['.', ' ']);
    let short: String = trimmed.chars().take(100).collect();
    let short = short.trim().trim_end_matches(['.', ' ']).to_string();
    if short.is_empty() {
        return "Playlist".into();
    }
    let stem = short.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = ["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str())
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && stem.as_bytes()[3].is_ascii_digit());
    if reserved {
        format!("_{short}")
    } else {
        short
    }
}

/// Options shared by every yt-dlp call. `--no-config` keeps the user's own
/// yt-dlp configuration from changing what MediaCraft does.
pub fn common_args(tools: &ToolPaths) -> Vec<String> {
    let mut a: Vec<String> = ["--no-config", "--encoding", "utf-8", "--socket-timeout", "30"]
        .map(String::from)
        .to_vec();
    if let Some(js) = &tools.js {
        a.push("--no-js-runtimes".into());
        a.push("--js-runtimes".into());
        a.push(format!("{}:{}", js.kind, js.path.display()));
    }
    if let Some(ff) = &tools.ffmpeg {
        if let Some(dir) = ff.parent() {
            a.push("--ffmpeg-location".into());
            a.push(dir.display().to_string());
        }
    }
    a
}

const PROGRESS_TEMPLATE: &str = "download:LG|dl|%(progress.status)s|%(progress.downloaded_bytes)s|%(progress.total_bytes)s|%(progress.total_bytes_estimate)s|%(progress.speed)s|%(progress.eta)s|%(info.vcodec)s|%(info.acodec)s";
const POSTPROCESS_TEMPLATE: &str = "postprocess:LG|pp|%(progress.status)s|%(progress.postprocessor)s";

pub fn download_args(spec: &DownloadSpec, url: &str, tools: &ToolPaths) -> Vec<String> {
    let mut a = common_args(tools);
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<String>>();

    a.extend(s(&[
        "--no-playlist",
        "--newline",
        "--progress",
        "--no-mtime",
        "--retries",
        "10",
        "--fragment-retries",
        "10",
        "--concurrent-fragments",
        "4",
        "--embed-metadata",
    ]));
    a.push("--progress-template".into());
    a.push(PROGRESS_TEMPLATE.into());
    a.push("--progress-template".into());
    a.push(POSTPROCESS_TEMPLATE.into());
    a.push("--print".into());
    a.push("after_move:LG|file|%(filepath)s".into());

    match spec.mode {
        Mode::Video => {
            let res = match spec.max_height {
                Some(h) => format!("res:{h}"),
                None => "res".to_string(),
            };
            let sort = match spec.container.as_str() {
                "mp4" => format!("{res},fps,ext:mp4:m4a"),
                "webm" => format!("{res},fps,ext:webm:webm"),
                _ => format!("{res},fps"),
            };
            a.extend(s(&["-f", "bv*+ba/b", "-S"]));
            a.push(sort);
            a.push("--merge-output-format".into());
            a.push(spec.container.clone());
            a.push("--remux-video".into());
            a.push(spec.container.clone());
        }
        Mode::Audio => {
            a.extend(s(&["-f", "ba/b", "-x", "--audio-format"]));
            a.push(spec.audio_format.clone());
            a.push("--audio-quality".into());
            let lossless = matches!(spec.audio_format.as_str(), "flac" | "wav");
            a.push(match spec.audio_bitrate {
                Some(b) if !lossless => format!("{b}K"),
                _ => "0".to_string(),
            });
            if ["mp3", "m4a", "opus", "flac"].contains(&spec.audio_format.as_str()) {
                a.push("--embed-thumbnail".into());
            }
        }
    }

    a.push("-P".into());
    a.push(spec.target_dir().display().to_string());
    a.push("-o".into());
    a.push(match spec.index {
        Some(i) => format!("{i:03} - %(title).170B [%(id)s].%(ext)s"),
        None => "%(title).180B [%(id)s].%(ext)s".to_string(),
    });

    // `--` stops a crafted link from being read as an option.
    a.push("--".into());
    a.push(url.trim().to_string());
    a
}

pub fn probe_args(url: &str, no_playlist: bool, tools: &ToolPaths) -> Vec<String> {
    let mut a = common_args(tools);
    a.extend(
        ["--dump-single-json", "--flat-playlist", "--no-warnings", "--skip-download"]
            .map(String::from),
    );
    a.push(if no_playlist { "--no-playlist" } else { "--yes-playlist" }.to_string());
    a.push("--".into());
    a.push(url.trim().to_string());
    a
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::JsRuntime;

    fn tools() -> ToolPaths {
        ToolPaths {
            ytdlp: Some(PathBuf::from(r"C:\t\yt-dlp.exe")),
            ffmpeg: Some(PathBuf::from(r"C:\t\ffmpeg.exe")),
            js: Some(JsRuntime { kind: "node".into(), path: PathBuf::from(r"C:\n\node.exe") }),
        }
    }

    fn spec(mode: Mode) -> DownloadSpec {
        DownloadSpec {
            mode,
            max_height: Some(1080),
            container: "mp4".into(),
            audio_format: "mp3".into(),
            audio_bitrate: None,
            folder: r"C:\Down".into(),
            subfolder: None,
            index: None,
        }
    }

    fn after<'a>(a: &'a [String], flag: &str) -> &'a str {
        let i = a.iter().position(|x| x == flag).unwrap_or_else(|| panic!("missing {flag}"));
        &a[i + 1]
    }

    #[test]
    fn video_caps_height_and_prefers_mp4() {
        let a = download_args(&spec(Mode::Video), "https://x.test/v", &tools());
        assert_eq!(after(&a, "-S"), "res:1080,fps,ext:mp4:m4a");
        assert_eq!(after(&a, "-f"), "bv*+ba/b");
        assert_eq!(after(&a, "--merge-output-format"), "mp4");
        assert!(a.contains(&"--no-playlist".to_string()));
    }

    #[test]
    fn best_quality_has_no_height_cap() {
        let mut s = spec(Mode::Video);
        s.max_height = None;
        s.container = "mkv".into();
        let a = download_args(&s, "https://x.test/v", &tools());
        assert_eq!(after(&a, "-S"), "res,fps");
    }

    #[test]
    fn audio_uses_extract_and_bitrate() {
        let mut s = spec(Mode::Audio);
        s.audio_bitrate = Some(192);
        let a = download_args(&s, "https://x.test/v", &tools());
        assert!(a.contains(&"-x".to_string()));
        assert_eq!(after(&a, "--audio-format"), "mp3");
        assert_eq!(after(&a, "--audio-quality"), "192K");
        assert!(a.contains(&"--embed-thumbnail".to_string()));
        assert!(!a.contains(&"--merge-output-format".to_string()));
    }

    #[test]
    fn lossless_ignores_bitrate_and_wav_skips_cover() {
        let mut s = spec(Mode::Audio);
        s.audio_format = "wav".into();
        s.audio_bitrate = Some(128);
        let a = download_args(&s, "https://x.test/v", &tools());
        assert_eq!(after(&a, "--audio-quality"), "0");
        assert!(!a.contains(&"--embed-thumbnail".to_string()));
    }

    #[test]
    fn link_is_always_after_double_dash() {
        let a = download_args(&spec(Mode::Video), "https://x.test/v", &tools());
        assert_eq!(a[a.len() - 2], "--");
        assert_eq!(a[a.len() - 1], "https://x.test/v");
        let p = probe_args("https://x.test/v", false, &tools());
        assert_eq!(p[p.len() - 2], "--");
    }

    #[test]
    fn passes_tool_locations_and_ignores_user_config() {
        let a = download_args(&spec(Mode::Video), "https://x.test/v", &tools());
        assert_eq!(a[0], "--no-config");
        assert_eq!(after(&a, "--js-runtimes"), r"node:C:\n\node.exe");
        assert_eq!(after(&a, "--ffmpeg-location"), r"C:\t");
    }

    #[test]
    fn playlist_gets_subfolder_and_number() {
        let mut s = spec(Mode::Video);
        s.subfolder = Some("My: List?".into());
        s.index = Some(7);
        let a = download_args(&s, "https://x.test/v", &tools());
        assert_eq!(after(&a, "-P"), r"C:\Down\My_ List_");
        assert!(after(&a, "-o").starts_with("007 - "));
    }

    #[test]
    fn sanitizes_folder_names() {
        assert_eq!(sanitize_component("a/b\\c:d"), "a_b_c_d");
        assert_eq!(sanitize_component("100% real."), "100_ real");
        assert_eq!(sanitize_component("   "), "Playlist");
        assert_eq!(sanitize_component("CON"), "_CON");
        assert_eq!(sanitize_component("COM1.txt"), "_COM1.txt");
        assert_eq!(sanitize_component("الذكاء"), "الذكاء");
        assert_eq!(sanitize_component(&"x".repeat(300)).chars().count(), 100);
    }

    #[test]
    fn rejects_bad_links() {
        assert!(validate_url("https://youtube.com/watch?v=1").is_ok());
        assert!(validate_url("HTTPS://youtube.com/").is_ok());
        assert!(validate_url("--exec calc").is_err());
        assert!(validate_url("file:///c:/x").is_err());
        assert!(validate_url("https://a b").is_err());
    }

    #[test]
    fn rejects_unknown_formats() {
        let mut s = spec(Mode::Video);
        assert!(validate_spec(&s).is_ok());
        s.container = "exe".into();
        assert!(validate_spec(&s).is_err());
    }
}

//! Turns yt-dlp's JSON into what the interface needs.

use serde::Serialize;
use serde_json::Value;

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct HeightOption {
    pub height: u32,
    pub fps: Option<f64>,
    /// Estimated size of the best stream at this height plus the best audio.
    pub size_estimate: Option<u64>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Entry {
    pub url: String,
    pub title: String,
    pub duration: Option<f64>,
    pub thumbnail: Option<String>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct ProbeResult {
    pub kind: String,
    pub url: String,
    pub title: String,
    pub uploader: Option<String>,
    pub duration: Option<f64>,
    pub thumbnail: Option<String>,
    pub heights: Vec<HeightOption>,
    pub audio_size_estimate: Option<u64>,
    pub entries: Vec<Entry>,
    pub skipped_entries: u32,
    /// The link names both one video and a playlist, so the user can choose.
    pub also_single_video: bool,
}

/// Playlists list removed or private videos without a title. The interface
/// matches this text to leave them unticked by default.
pub const UNTITLED: &str = "Unavailable or untitled video";

fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).filter(|s| !s.is_empty()).map(String::from)
}

fn size_of(f: &Value) -> Option<u64> {
    f.get("filesize")
        .and_then(Value::as_f64)
        .or_else(|| f.get("filesize_approx").and_then(Value::as_f64))
        .filter(|v| *v > 0.0)
        .map(|v| v as u64)
}

fn has_codec(f: &Value, key: &str) -> bool {
    matches!(f.get(key).and_then(Value::as_str), Some(c) if c != "none")
}

fn best_thumbnail(v: &Value) -> Option<String> {
    text(v, "thumbnail").or_else(|| {
        v.get("thumbnails")
            .and_then(Value::as_array)
            .and_then(|t| t.iter().rev().find_map(|x| text(x, "url")))
    })
}

fn collect_entries(v: &Value, out: &mut Vec<Entry>, skipped: &mut u32) {
    let Some(list) = v.get("entries").and_then(Value::as_array) else {
        return;
    };
    for e in list {
        if e.is_null() {
            *skipped += 1;
            continue;
        }
        if e.get("entries").is_some() {
            collect_entries(e, out, skipped);
            continue;
        }
        let url = text(e, "url").or_else(|| text(e, "webpage_url"));
        match url {
            Some(url) if url.starts_with("http") => out.push(Entry {
                url,
                title: text(e, "title").unwrap_or_else(|| UNTITLED.into()),
                duration: e.get("duration").and_then(Value::as_f64),
                thumbnail: best_thumbnail(e),
            }),
            _ => *skipped += 1,
        }
    }
}

fn link_names_video_and_list(url: &str) -> bool {
    let q = url.split_once('?').map(|x| x.1).unwrap_or("");
    let has = |k: &str| q.split('&').any(|p| p.starts_with(k));
    has("v=") && has("list=")
}

pub fn parse(json: &str, url: &str) -> Result<ProbeResult, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("Couldn't read yt-dlp's answer: {e}"))?;
    let is_list = v.get("entries").is_some();

    let mut base = ProbeResult {
        kind: if is_list { "playlist" } else { "video" }.to_string(),
        url: url.trim().to_string(),
        title: text(&v, "title").unwrap_or_else(|| "Untitled".into()),
        uploader: text(&v, "uploader").or_else(|| text(&v, "channel")),
        duration: v.get("duration").and_then(Value::as_f64),
        thumbnail: best_thumbnail(&v),
        heights: vec![],
        audio_size_estimate: None,
        entries: vec![],
        skipped_entries: 0,
        also_single_video: false,
    };

    if is_list {
        collect_entries(&v, &mut base.entries, &mut base.skipped_entries);
        base.also_single_video = link_names_video_and_list(url);
        if base.thumbnail.is_none() {
            base.thumbnail = base.entries.iter().find_map(|e| e.thumbnail.clone());
        }
        return Ok(base);
    }

    let empty = vec![];
    let formats = v.get("formats").and_then(Value::as_array).unwrap_or(&empty);

    base.audio_size_estimate = formats
        .iter()
        .filter(|f| has_codec(f, "acodec") && !has_codec(f, "vcodec"))
        .filter_map(size_of)
        .max();

    let mut heights: Vec<HeightOption> = vec![];
    for f in formats.iter().filter(|f| has_codec(f, "vcodec")) {
        let Some(h) = f.get("height").and_then(Value::as_u64).filter(|h| *h > 0) else {
            continue;
        };
        let h = h as u32;
        let fps = f.get("fps").and_then(Value::as_f64);
        let muxed = has_codec(f, "acodec");
        let size = size_of(f).map(|s| s + if muxed { 0 } else { base.audio_size_estimate.unwrap_or(0) });
        match heights.iter_mut().find(|o| o.height == h) {
            Some(o) => {
                if fps > o.fps {
                    o.fps = fps;
                }
                if size > o.size_estimate {
                    o.size_estimate = size;
                }
            }
            None => heights.push(HeightOption { height: h, fps, size_estimate: size }),
        }
    }
    heights.sort_by(|a, b| b.height.cmp(&a.height));
    base.heights = heights;
    Ok(base)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_lists_heights_high_to_low_with_size_including_audio() {
        let j = r#"{"title":"T","uploader":"U","duration":19,"thumbnail":"https://i/t.jpg","formats":[
          {"format_id":"a","vcodec":"none","acodec":"opus","filesize":100},
          {"format_id":"sb","vcodec":"none","acodec":"none","height":45},
          {"format_id":"v1","vcodec":"avc1","acodec":"none","height":720,"fps":30,"filesize":1000},
          {"format_id":"v2","vcodec":"vp9","acodec":"none","height":1080,"fps":60,"filesize_approx":5000},
          {"format_id":"m","vcodec":"avc1","acodec":"aac","height":360,"fps":30,"filesize":300}
        ]}"#;
        let r = parse(j, "https://x/v").unwrap();
        assert_eq!(r.kind, "video");
        assert_eq!(r.heights.iter().map(|h| h.height).collect::<Vec<_>>(), [1080, 720, 360]);
        assert_eq!(r.heights[0].size_estimate, Some(5100));
        assert_eq!(r.heights[0].fps, Some(60.0));
        assert_eq!(r.heights[2].size_estimate, Some(300), "already has audio");
        assert_eq!(r.audio_size_estimate, Some(100));
    }

    #[test]
    fn playlist_keeps_order_and_counts_unusable_entries() {
        let j = r#"{"_type":"playlist","title":"L","entries":[
          {"url":"https://x/1","title":"A","duration":5},
          null,
          {"title":"no link"},
          {"url":"https://x/2","title":"B","thumbnails":[{"url":"https://i/s"},{"url":"https://i/l"}]}
        ]}"#;
        let r = parse(j, "https://x/playlist?list=1").unwrap();
        assert_eq!(r.kind, "playlist");
        assert_eq!(r.entries.len(), 2);
        assert_eq!(r.entries[0].title, "A");
        assert_eq!(r.entries[1].thumbnail.as_deref(), Some("https://i/l"));
        assert_eq!(r.skipped_entries, 2);
        assert!(!r.also_single_video);
    }

    #[test]
    fn channel_tabs_are_flattened() {
        let j = r#"{"title":"C","entries":[
          {"title":"Videos","entries":[{"url":"https://x/1","title":"A"}]},
          {"title":"Shorts","entries":[{"url":"https://x/2","title":"B"}]}
        ]}"#;
        assert_eq!(parse(j, "https://x/@c").unwrap().entries.len(), 2);
    }

    #[test]
    fn detects_link_with_video_and_list() {
        let j = r#"{"title":"L","entries":[]}"#;
        assert!(parse(j, "https://y/watch?v=1&list=2").unwrap().also_single_video);
        assert!(parse(j, "https://y/watch?list=2&v=1").unwrap().also_single_video);
        assert!(!parse(j, "https://y/playlist?list=2").unwrap().also_single_video);
    }

    #[test]
    fn bad_json_is_an_error_not_a_panic() {
        assert!(parse("not json", "https://x").is_err());
    }
}

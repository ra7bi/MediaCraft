//! Parses yt-dlp's machine-readable lines and turns its errors into plain advice.

#[derive(Debug, PartialEq)]
pub enum Line {
    Download {
        status: String,
        downloaded: Option<f64>,
        total: Option<f64>,
        speed: Option<f64>,
        eta: Option<f64>,
        is_audio: bool,
    },
    PostProcess {
        status: String,
        name: String,
    },
    File(String),
    Other,
}

fn num(s: &str) -> Option<f64> {
    s.trim().parse::<f64>().ok().filter(|v| v.is_finite() && *v >= 0.0)
}

pub fn parse_line(line: &str) -> Line {
    let line = line.trim_end_matches(['\r', '\n']);
    let Some(rest) = line.strip_prefix("LG|") else {
        return Line::Other;
    };
    if let Some(path) = rest.strip_prefix("file|") {
        return Line::File(path.to_string());
    }
    if let Some(pp) = rest.strip_prefix("pp|") {
        let mut it = pp.splitn(2, '|');
        return Line::PostProcess {
            status: it.next().unwrap_or("").to_string(),
            name: it.next().unwrap_or("").to_string(),
        };
    }
    if let Some(dl) = rest.strip_prefix("dl|") {
        let f: Vec<&str> = dl.split('|').collect();
        if f.len() < 8 {
            return Line::Other;
        }
        let total = num(f[2]).or_else(|| num(f[3]));
        return Line::Download {
            status: f[0].to_string(),
            downloaded: num(f[1]),
            total,
            speed: num(f[4]),
            eta: num(f[5]),
            is_audio: f[6] == "none" && f[7] != "none",
        };
    }
    Line::Other
}

pub fn postprocess_label(name: &str) -> String {
    match name {
        "Merger" => "Merging video and audio",
        "ExtractAudio" => "Converting audio",
        "VideoRemuxer" => "Finishing the video file",
        "EmbedThumbnail" => "Adding cover art",
        "Metadata" => "Adding title and details",
        _ => "Finishing up",
    }
    .to_string()
}

/// Picks the real error out of yt-dlp's stderr and explains it.
pub fn friendly_error(stderr: &str, exit_code: Option<i32>) -> String {
    let raw = stderr
        .lines()
        .rev()
        .find(|l| l.trim_start().starts_with("ERROR:"))
        .map(|l| l.trim_start().trim_start_matches("ERROR:").trim().to_string());

    let hay = raw.as_deref().unwrap_or(stderr).to_lowercase();
    let has = |n: &str| hay.contains(n);

    let advice = if has("drm") {
        "This video is DRM-protected, and MediaCraft doesn't download protected content."
    } else if has("private video") {
        "This video is private."
    } else if has("confirm your age") || has("age-restricted") {
        "This video is age-restricted and needs a signed-in account, which MediaCraft doesn't support yet."
    } else if has("not a bot") {
        "YouTube is asking for a bot check. Try again later, and update yt-dlp in Components."
    } else if has("members-only") || has("join this channel") {
        "This video is for channel members only."
    } else if has("video unavailable") || has("video is unavailable") || has("this video is not available") {
        "This video is unavailable. It may be removed or blocked in your country."
    } else if has("unsupported url") || has("is not a valid url") {
        "This site or link isn't supported."
    } else if has("requested format is not available") {
        "That quality isn't available for this video. Try Best."
    } else if has("no space left") || has("not enough space") || has("errno 28") {
        "The disk is full. Free up space or choose another folder."
    } else if has("permission denied") || has("errno 13") || has("access is denied") {
        "MediaCraft can't write to that folder. Choose another one, or close the file if it's open elsewhere."
    } else if has("ffmpeg") && (has("not found") || has("not installed")) {
        "FFmpeg is missing. Install it from Components."
    } else if has("http error 403") {
        "The server refused the download (HTTP 403). Update yt-dlp in Components, then retry."
    } else if has("http error 429") {
        "The site is limiting requests (HTTP 429). Wait a while, then retry."
    } else if has("getaddrinfo") || has("timed out") || has("connection") || has("network is unreachable") {
        "Network problem. Check your connection and the link, then retry."
    } else {
        ""
    };

    match (advice.is_empty(), raw) {
        (false, _) => advice.to_string(),
        (true, Some(r)) => r,
        (true, None) => match exit_code {
            Some(c) => format!("yt-dlp stopped unexpectedly (exit code {c})."),
            None => "yt-dlp stopped unexpectedly.".to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_progress() {
        let l = parse_line("LG|dl|downloading|1048576|418567807|NA|17000000.5|24|avc1.64|none\r\n");
        assert_eq!(
            l,
            Line::Download {
                status: "downloading".into(),
                downloaded: Some(1048576.0),
                total: Some(418567807.0),
                speed: Some(17000000.5),
                eta: Some(24.0),
                is_audio: false,
            }
        );
    }

    #[test]
    fn falls_back_to_estimated_size_and_handles_na() {
        match parse_line("LG|dl|downloading|10|NA|500.0|NA|NA|none|mp4a.40.2") {
            Line::Download { total, speed, eta, is_audio, .. } => {
                assert_eq!(total, Some(500.0));
                assert_eq!(speed, None);
                assert_eq!(eta, None);
                assert!(is_audio);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn parses_file_with_unicode_and_pipes() {
        assert_eq!(
            parse_line("LG|file|C:\\Down\\الذكاء | x.mp4"),
            Line::File("C:\\Down\\الذكاء | x.mp4".into())
        );
    }

    #[test]
    fn parses_postprocess() {
        assert_eq!(
            parse_line("LG|pp|started|Merger"),
            Line::PostProcess { status: "started".into(), name: "Merger".into() }
        );
        assert_eq!(postprocess_label("Merger"), "Merging video and audio");
    }

    #[test]
    fn ignores_other_output() {
        assert_eq!(parse_line("[youtube] Extracting URL"), Line::Other);
        assert_eq!(parse_line("LG|dl|short"), Line::Other);
    }

    #[test]
    fn explains_known_errors() {
        let e = friendly_error("WARNING: x\nERROR: [youtube] abc: Private video. Sign in", Some(1));
        assert_eq!(e, "This video is private.");
        let e = friendly_error("ERROR: unable to download video data: HTTP Error 403: Forbidden", Some(1));
        assert!(e.contains("403"));
        let e = friendly_error("ERROR: [Foo] This video is DRM protected", Some(1));
        assert!(e.contains("DRM"));
        let e = friendly_error("ERROR: [youtube] aaa: This video is unavailable", Some(1));
        assert!(e.contains("removed or blocked"), "{e}");
    }

    #[test]
    fn keeps_unknown_error_text_never_unknown_error() {
        let e = friendly_error("ERROR: something odd happened", Some(1));
        assert_eq!(e, "something odd happened");
        assert_eq!(friendly_error("", Some(3)), "yt-dlp stopped unexpectedly (exit code 3).");
    }
}

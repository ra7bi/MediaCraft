//! Finds yt-dlp, FFmpeg and a JavaScript runtime, and installs them from their
//! official releases after checking the published SHA-256.

use serde::Serialize;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Builds a command that never flashes a console window.
pub fn hidden_command(program: &Path) -> Command {
    #[allow(unused_mut)]
    let mut c = Command::new(program);
    #[cfg(windows)]
    c.creation_flags(CREATE_NO_WINDOW);
    c
}

#[derive(Clone, Debug, PartialEq)]
pub struct JsRuntime {
    /// The name yt-dlp uses: deno, node, quickjs or bun.
    pub kind: String,
    pub path: PathBuf,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolPaths {
    pub ytdlp: Option<PathBuf>,
    pub ffmpeg: Option<PathBuf>,
    pub js: Option<JsRuntime>,
}

fn search_path(file: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths).map(|d| d.join(file)).find(|p| p.is_file())
}

fn find(file: &str, tools_dir: &Path) -> Option<PathBuf> {
    let local = tools_dir.join(file);
    if local.is_file() {
        return Some(local);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(beside) = exe.parent().map(|d| d.join(file)).filter(|p| p.is_file()) {
            return Some(beside);
        }
    }
    search_path(file)
}

/// The folder MediaCraft installs into. `MEDIACRAFT_TOOLS_DIR` overrides it for tests.
pub fn effective_tools_dir(default_dir: &Path) -> PathBuf {
    std::env::var_os("MEDIACRAFT_TOOLS_DIR").map(PathBuf::from).unwrap_or_else(|| default_dir.to_path_buf())
}

pub fn resolve(tools_dir: &Path) -> ToolPaths {
    let js = [("deno", "deno.exe"), ("quickjs", "qjs.exe"), ("node", "node.exe"), ("bun", "bun.exe")]
        .iter()
        .find_map(|(kind, file)| {
            find(file, tools_dir).map(|path| JsRuntime { kind: kind.to_string(), path })
        });
    ToolPaths { ytdlp: find("yt-dlp.exe", tools_dir), ffmpeg: find("ffmpeg.exe", tools_dir), js }
}

#[derive(Clone, Debug, Serialize)]
pub struct ToolInfo {
    pub id: String,
    pub label: String,
    pub purpose: String,
    pub found: bool,
    pub path: Option<String>,
    pub version: Option<String>,
    pub managed: bool,
    pub outdated: bool,
    pub can_install: bool,
    pub download_note: String,
}

fn first_line_of(program: &Path, args: &[&str]) -> Option<String> {
    let out = hidden_command(program).args(args).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines().next().map(|l| l.trim().to_string()).filter(|l| !l.is_empty())
}

/// yt-dlp versions are dates, such as 2026.10.26. Returns days since that date.
pub fn version_age_days(version: &str, today_days: i64) -> Option<i64> {
    let mut it = version.split('.');
    let y: i64 = it.next()?.parse().ok()?;
    let m: i64 = it.next()?.parse().ok()?;
    let d: i64 = it.next()?.parse().ok()?;
    if !(2000..3000).contains(&y) || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some(today_days - days_from_civil(y, m, d))
}

/// Days since 1970-01-01 for a calendar date.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn today_days() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| (d.as_secs() / 86400) as i64)
        .unwrap_or(0)
}

const OUTDATED_AFTER_DAYS: i64 = 90;

pub fn status(tools_dir: &Path) -> Vec<ToolInfo> {
    let t = resolve(tools_dir);
    let managed = |p: &Option<PathBuf>| p.as_ref().is_some_and(|p| p.starts_with(tools_dir));

    let yt_version = t.ytdlp.as_ref().and_then(|p| first_line_of(p, &["--version"]));
    let outdated = yt_version
        .as_deref()
        .and_then(|v| version_age_days(v, today_days()))
        .is_some_and(|age| age > OUTDATED_AFTER_DAYS);
    let ff_version = t.ffmpeg.as_ref().and_then(|p| first_line_of(p, &["-version"])).map(|l| {
        l.split_whitespace().nth(2).unwrap_or(&l).split("-www").next().unwrap_or("").to_string()
    });
    let js_path = t.js.as_ref().map(|j| j.path.clone());
    let js_version = t.js.as_ref().and_then(|j| first_line_of(&j.path, &["--version"]));

    vec![
        ToolInfo {
            id: "yt-dlp".into(),
            label: "yt-dlp".into(),
            purpose: "Finds videos and downloads them".into(),
            found: t.ytdlp.is_some(),
            path: t.ytdlp.as_ref().map(|p| p.display().to_string()),
            version: yt_version,
            managed: managed(&t.ytdlp),
            outdated,
            can_install: true,
            download_note: "About 24 MB, from github.com/yt-dlp/yt-dlp".into(),
        },
        ToolInfo {
            id: "ffmpeg".into(),
            label: "FFmpeg".into(),
            purpose: "Merges video and audio, and converts audio".into(),
            found: t.ffmpeg.is_some(),
            path: t.ffmpeg.as_ref().map(|p| p.display().to_string()),
            version: ff_version,
            managed: managed(&t.ffmpeg),
            outdated: false,
            can_install: true,
            download_note: "About 91 MB download, from github.com/yt-dlp/FFmpeg-Builds".into(),
        },
        ToolInfo {
            id: "js".into(),
            label: t.js.as_ref().map(|j| format!("JavaScript runtime ({})", j.kind)).unwrap_or_else(|| "JavaScript runtime".into()),
            purpose: "Needed by yt-dlp for full YouTube support".into(),
            found: t.js.is_some(),
            managed: managed(&js_path),
            path: js_path.map(|p| p.display().to_string()),
            version: js_version,
            outdated: false,
            can_install: true,
            download_note: "Deno, about 43 MB download, from github.com/denoland/deno".into(),
        },
    ]
}

struct Package {
    url: &'static str,
    sums_url: &'static str,
    /// The name to look for in the checksum file.
    sums_name: &'static str,
    /// Plain executable name, or `None` when the download is a zip.
    plain_file: Option<&'static str>,
}

fn package(id: &str) -> Option<Package> {
    match id {
        "yt-dlp" => Some(Package {
            url: "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe",
            sums_url: "https://github.com/yt-dlp/yt-dlp/releases/latest/download/SHA2-256SUMS",
            sums_name: "yt-dlp.exe",
            plain_file: Some("yt-dlp.exe"),
        }),
        "ffmpeg" => Some(Package {
            url: "https://github.com/yt-dlp/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl-shared.zip",
            sums_url: "https://github.com/yt-dlp/FFmpeg-Builds/releases/download/latest/checksums.sha256",
            sums_name: "ffmpeg-master-latest-win64-gpl-shared.zip",
            plain_file: None,
        }),
        "js" => Some(Package {
            url: "https://github.com/denoland/deno/releases/latest/download/deno-x86_64-pc-windows-msvc.zip",
            sums_url: "https://github.com/denoland/deno/releases/latest/download/deno-x86_64-pc-windows-msvc.zip.sha256sum",
            sums_name: "deno-x86_64-pc-windows-msvc.zip",
            plain_file: None,
        }),
        _ => None,
    }
}

/// Finds the hash for `name` in a checksum file. Hash files may hold one hash
/// or many lines, with the name before or after the hash.
pub fn find_checksum(sums: &str, name: &str) -> Option<String> {
    let is_hash = |t: &str| t.len() == 64 && t.chars().all(|c| c.is_ascii_hexdigit());
    let on_named_line = sums
        .lines()
        .filter(|l| l.split_whitespace().any(|t| t.trim_start_matches('*').rsplit(['/', '\\']).next() == Some(name)))
        .find_map(|l| l.split_whitespace().find(|t| is_hash(t)));
    let any_single = || {
        let all: Vec<&str> = sums.split_whitespace().filter(|t| is_hash(t)).collect();
        if all.len() == 1 { Some(all[0]) } else { None }
    };
    on_named_line.or_else(any_single).map(|h| h.to_ascii_lowercase())
}

/// Download, verify, then unpack. Pure enough to test with local data.
fn install_verified(
    pkg_plain: Option<&str>,
    temp_file: &Path,
    expected: &str,
    actual: &str,
    tools_dir: &Path,
) -> Result<(), String> {
    if !expected.eq_ignore_ascii_case(actual) {
        let _ = std::fs::remove_file(temp_file);
        return Err("The download didn't match its published checksum, so it was discarded.".into());
    }
    match pkg_plain {
        Some(file) => {
            let dest = tools_dir.join(file);
            std::fs::rename(temp_file, &dest).map_err(|e| format!("Couldn't install {file}: {e}"))
        }
        None => {
            let r = unpack_binaries(temp_file, tools_dir);
            let _ = std::fs::remove_file(temp_file);
            r
        }
    }
}

/// Extracts only .exe and .dll files, flattened, so nothing can escape the folder.
pub fn unpack_binaries(zip_path: &Path, tools_dir: &Path) -> Result<(), String> {
    let f = std::fs::File::open(zip_path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(f).map_err(|e| format!("Couldn't open the archive: {e}"))?;
    let mut extracted = 0;
    for i in 0..archive.len() {
        let mut item = archive.by_index(i).map_err(|e| e.to_string())?;
        if item.is_dir() {
            continue;
        }
        let name = item.name().replace('\\', "/");
        let file = name.rsplit('/').next().unwrap_or("").to_string();
        let lower = file.to_ascii_lowercase();
        if file.is_empty() || !(lower.ends_with(".exe") || lower.ends_with(".dll")) {
            continue;
        }
        let dest = tools_dir.join(&file);
        let tmp = tools_dir.join(format!("{file}.new"));
        let mut out = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        std::io::copy(&mut item, &mut out).map_err(|e| e.to_string())?;
        drop(out);
        std::fs::rename(&tmp, &dest).map_err(|e| format!("Couldn't install {file}: {e}"))?;
        extracted += 1;
    }
    if extracted == 0 {
        return Err("The archive didn't contain any programs.".into());
    }
    Ok(())
}

/// Installs a tool. `progress(downloaded, total)` is called during the download.
pub fn install(id: &str, tools_dir: &Path, progress: &dyn Fn(u64, Option<u64>)) -> Result<(), String> {
    let pkg = package(id).ok_or_else(|| format!("Unknown component: {id}"))?;
    std::fs::create_dir_all(tools_dir).map_err(|e| format!("Couldn't create the tools folder: {e}"))?;

    let sums = ureq::get(pkg.sums_url)
        .call()
        .map_err(|e| format!("Couldn't get the checksum: {e}"))?
        .into_string()
        .map_err(|e| format!("Couldn't read the checksum: {e}"))?;
    let expected = find_checksum(&sums, pkg.sums_name)
        .ok_or_else(|| "The checksum file didn't list this download, so it wasn't installed.".to_string())?;

    let resp = ureq::get(pkg.url).call().map_err(|e| format!("Download failed: {e}"))?;
    let total = resp.header("Content-Length").and_then(|v| v.parse::<u64>().ok());
    let temp_file = tools_dir.join(format!("{id}.download"));
    let mut out = std::fs::File::create(&temp_file).map_err(|e| e.to_string())?;
    let mut reader = resp.into_reader();
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    let mut done = 0u64;
    loop {
        let n = reader.read(&mut buf).map_err(|e| format!("Download failed: {e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        done += n as u64;
        progress(done, total);
    }
    drop(out);
    let actual: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    install_verified(pkg.plain_file, &temp_file, &expected, &actual, tools_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: &str = "f5389d3698ecde4ef06f4e6b1ce0294fe9ac1396228c8990b8d3416d6d177a7d";

    fn temp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("mediacraft-tools-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn finds_hash_in_multi_line_file() {
        let other = "a".repeat(64);
        let s = format!("{other}  yt-dlp_linux\n{H}  yt-dlp.exe\n");
        assert_eq!(find_checksum(&s, "yt-dlp.exe").as_deref(), Some(H));
    }

    #[test]
    fn finds_hash_with_star_or_path_prefix() {
        assert_eq!(find_checksum(&format!("{H} *x.zip"), "x.zip").as_deref(), Some(H));
        assert_eq!(find_checksum(&format!("{H}  build/x.zip"), "x.zip").as_deref(), Some(H));
    }

    #[test]
    fn single_hash_file_without_name_is_accepted() {
        assert_eq!(find_checksum(&format!("Hash : {}\n", H.to_uppercase()), "x.zip").as_deref(), Some(H));
    }

    #[test]
    fn ambiguous_or_missing_hash_is_refused() {
        let s = format!("{H}  a.exe\n{}  b.exe\n", "b".repeat(64));
        assert_eq!(find_checksum(&s, "c.exe"), None);
        assert_eq!(find_checksum("nothing here", "c.exe"), None);
    }

    #[test]
    fn checksum_mismatch_discards_the_file() {
        let d = temp("mismatch");
        let f = d.join("t.download");
        std::fs::write(&f, b"bad").unwrap();
        let r = install_verified(Some("yt-dlp.exe"), &f, &"a".repeat(64), &"b".repeat(64), &d);
        assert!(r.is_err());
        assert!(!f.exists());
        assert!(!d.join("yt-dlp.exe").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn checksum_match_installs_the_file() {
        let d = temp("match");
        let f = d.join("t.download");
        std::fs::write(&f, b"ok").unwrap();
        install_verified(Some("yt-dlp.exe"), &f, H, H, &d).unwrap();
        assert!(d.join("yt-dlp.exe").is_file());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn unpack_keeps_programs_only_and_cannot_escape() {
        use zip::write::SimpleFileOptions;
        let d = temp("zip");
        let z = d.join("a.zip");
        let mut w = zip::ZipWriter::new(std::fs::File::create(&z).unwrap());
        let o = SimpleFileOptions::default();
        for (n, b) in [
            ("pkg/bin/ffmpeg.exe", "exe"),
            ("pkg/bin/avcodec.dll", "dll"),
            ("pkg/doc/readme.txt", "txt"),
            ("../../evil.exe", "evil"),
        ] {
            w.start_file(n, o).unwrap();
            w.write_all(b.as_bytes()).unwrap();
        }
        w.finish().unwrap();
        let out = d.join("out");
        std::fs::create_dir_all(&out).unwrap();
        unpack_binaries(&z, &out).unwrap();
        assert!(out.join("ffmpeg.exe").is_file());
        assert!(out.join("avcodec.dll").is_file());
        assert!(!out.join("readme.txt").exists());
        assert!(out.join("evil.exe").is_file(), "flattened into the folder");
        assert!(!d.parent().unwrap().join("evil.exe").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn version_age_is_computed_from_the_date() {
        let today = days_from_civil(2026, 11, 8);
        assert_eq!(version_age_days("2026.10.26", today), Some(13));
        assert!(version_age_days("2025.05.22", today).unwrap() > OUTDATED_AFTER_DAYS);
        assert_eq!(version_age_days("nightly", today), None);
    }
}

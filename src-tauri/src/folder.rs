//! Checks the save folder before any network work happens.

use serde::Serialize;
use std::path::{Component, Path, PathBuf};

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct FolderCheck {
    pub ok: bool,
    pub message: String,
    pub free_bytes: Option<u64>,
}

fn nearest_existing(path: &Path) -> Option<PathBuf> {
    let mut cur = Some(path);
    while let Some(p) = cur {
        if p.exists() {
            return Some(p.to_path_buf());
        }
        cur = p.parent();
    }
    None
}

fn drive_name(path: &Path) -> Option<String> {
    match path.components().next() {
        Some(Component::Prefix(p)) => Some(p.as_os_str().to_string_lossy().to_string()),
        _ => None,
    }
}

fn fail(message: String) -> FolderCheck {
    FolderCheck { ok: false, message, free_bytes: None }
}

/// With `create`, makes the folder. Without it, nothing is left behind.
pub fn check(path: &Path, create: bool) -> FolderCheck {
    if path.as_os_str().is_empty() {
        return fail("Choose a folder to save into.".into());
    }
    if !path.is_absolute() {
        return fail("Choose a full folder path.".into());
    }
    let Some(existing) = nearest_existing(path) else {
        return fail(match drive_name(path) {
            Some(d) => format!("Drive {d} isn't available on this PC. Choose another folder."),
            None => "This location isn't available. Choose another folder.".to_string(),
        });
    };
    if !existing.is_dir() {
        return fail(format!("{} is a file, not a folder.", existing.display()));
    }
    if create && !path.exists() {
        if let Err(e) = std::fs::create_dir_all(path) {
            return fail(format!("Couldn't create the folder: {e}"));
        }
    }
    let test_dir = if path.exists() { path.to_path_buf() } else { existing.clone() };
    let probe = test_dir.join(format!(".mediacraft-write-test-{}", std::process::id()));
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
        }
        Err(e) => return fail(format!("MediaCraft can't write to this folder: {e}")),
    }
    FolderCheck {
        ok: true,
        message: if path.exists() {
            String::new()
        } else {
            "This folder will be created.".to_string()
        },
        free_bytes: fs4::available_space(&test_dir).ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("mediacraft-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    fn missing_drive() -> Option<String> {
        ('D'..='Z').map(|c| format!("{c}:\\")).find(|d| !Path::new(d).exists())
    }

    #[test]
    fn missing_drive_is_reported_by_name() {
        let d = missing_drive().expect("a free drive letter");
        let c = check(&Path::new(&d).join("Youtube").join("Courses"), false);
        assert!(!c.ok);
        assert!(c.message.contains(&d[..2]), "{}", c.message);
    }

    #[test]
    fn missing_folder_on_real_drive_is_fine_and_not_created_by_a_check() {
        let p = temp("nocreate").join("a").join("b");
        let c = check(&p, false);
        assert!(c.ok, "{}", c.message);
        assert!(c.message.contains("created"));
        assert!(!p.exists());
    }

    #[test]
    fn create_makes_the_folder_and_reports_space() {
        let base = temp("create");
        let p = base.join("x");
        let c = check(&p, true);
        assert!(c.ok, "{}", c.message);
        assert!(p.is_dir());
        assert!(c.free_bytes.unwrap_or(0) > 0);
        assert_eq!(std::fs::read_dir(&p).unwrap().count(), 0, "write test left a file");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_file_is_not_a_folder() {
        let base = temp("file");
        std::fs::create_dir_all(&base).unwrap();
        let f = base.join("f.txt");
        std::fs::write(&f, b"x").unwrap();
        let c = check(&f, false);
        assert!(!c.ok);
        assert!(c.message.contains("file"));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn empty_and_relative_paths_are_rejected() {
        assert!(!check(Path::new(""), false).ok);
        assert!(!check(Path::new("relative\\path"), false).ok);
    }
}

//! Cache katalogów, dzięki któremu sklep startuje natychmiast.
//!
//! Cache jest celowo prostym TSV — wczytywanie 100 000 pakietów to jedno
//! przejście po pliku, bez deserializacji JSON-a i bez zależności.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::model::{BackendKind, RawPackage};

const HEADER: &str = "# luna-store catalog v1";

pub fn cache_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_CACHE_HOME").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(dir).join("luna/store"));
    }
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".cache/luna/store"))
}

fn catalog_path(source: &str) -> Option<PathBuf> {
    cache_dir().map(|dir| dir.join(format!("{}.catalog", safe_name(source))))
}

fn stamp_path(source: &str) -> Option<PathBuf> {
    cache_dir().map(|dir| dir.join(format!("{}.stamp", safe_name(source))))
}

fn notified_path() -> Option<PathBuf> {
    cache_dir().map(|dir| dir.join("notified-updates"))
}

/// Liczba aktualizacji, o których powiadomiliśmy w poprzednich uruchomieniach.
///
/// Dzięki temu ponowne otwarcie Sklepu nie powtarza tego samego powiadomienia.
pub fn load_notified_updates() -> usize {
    notified_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| text.trim().parse().ok())
        .unwrap_or(0)
}

pub fn save_notified_updates(count: usize) {
    let Some(path) = notified_path() else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, count.to_string());
}

fn safe_name(source: &str) -> String {
    source
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

pub fn load(source: &str, backend: BackendKind) -> Vec<RawPackage> {
    let Some(path) = catalog_path(source) else {
        return Vec::new();
    };
    load_from(&path, backend)
}

fn load_from(path: &Path, backend: BackendKind) -> Vec<RawPackage> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut lines = text.lines();
    let expected = format!("{HEADER}\t{backend}");
    if lines.next().map(str::trim_end) != Some(expected.as_str()) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for line in lines {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        let mut fields = line.splitn(5, '\t');
        let Some(id) = fields.next().filter(|id| !id.is_empty()) else {
            continue;
        };
        let name = fields.next().unwrap_or(id);
        let version = fields.next().unwrap_or("");
        let description = fields.next().unwrap_or("");
        let remote = fields.next().filter(|remote| !remote.is_empty());
        out.push(RawPackage {
            id: id.to_owned(),
            name: if name.is_empty() { id } else { name }.to_owned(),
            version: version.to_owned(),
            description: description.to_owned(),
            remote: remote.map(str::to_owned),
        });
    }
    out
}

pub fn save(source: &str, backend: BackendKind, packages: &[RawPackage]) {
    let Some(path) = catalog_path(source) else {
        return;
    };
    if save_to(&path, backend, packages).is_ok() {
        write_stamp(source);
    }
}

fn save_to(path: &Path, backend: BackendKind, packages: &[RawPackage]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut text = String::with_capacity(packages.len() * 96 + 64);
    text.push_str(&format!("{HEADER}\t{backend}\n"));
    for package in packages {
        // Pola nie mogą zawierać tabulatorów ani nowych linii.
        text.push_str(&escape(&package.id));
        text.push('\t');
        text.push_str(&escape(&package.name));
        text.push('\t');
        text.push_str(&escape(&package.version));
        text.push('\t');
        text.push_str(&escape(&package.description));
        text.push('\t');
        if let Some(remote) = &package.remote {
            text.push_str(&escape(remote));
        }
        text.push('\n');
    }
    let tmp = path.with_extension("catalog.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(tmp, path)
}

fn escape(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch == '\t' || ch == '\n' || ch == '\r' {
                ' '
            } else {
                ch
            }
        })
        .collect()
}

fn write_stamp(source: &str) {
    let Some(path) = stamp_path(source) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let _ = std::fs::write(path, now.to_string());
}

pub fn stamp(source: &str) -> Option<SystemTime> {
    let path = stamp_path(source)?;
    let text = std::fs::read_to_string(path).ok()?;
    let seconds = text.trim().parse::<u64>().ok()?;
    Some(UNIX_EPOCH + std::time::Duration::from_secs(seconds))
}

pub fn is_stale(source: &str, refresh_minutes: u64) -> bool {
    let Some(modified) = stamp(source) else {
        return true;
    };
    match SystemTime::now().duration_since(modified) {
        Ok(age) => age.as_secs() >= refresh_minutes.saturating_mul(60),
        Err(_) => true,
    }
}

/// Usuwa znacznik świeżości (wymusza odświeżenie przy następnym starcie).
pub fn invalidate(source: &str) {
    if let Some(path) = stamp_path(source) {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("luna-store-cache-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn roundtrip_preserves_packages() {
        let dir = temp_dir("roundtrip");
        let path = dir.join("source.catalog");
        let packages = vec![
            RawPackage {
                id: "firefox".into(),
                name: "Firefox".into(),
                version: "130.0".into(),
                description: "Web browser".into(),
                remote: None,
            },
            RawPackage {
                id: "org.gimp.GIMP".into(),
                name: "GIMP".into(),
                version: "2.10".into(),
                description: "Image editor".into(),
                remote: Some("flathub".into()),
            },
        ];
        save_to(&path, BackendKind::Pacman, &packages).unwrap();
        let loaded = load_from(&path, BackendKind::Pacman);
        assert_eq!(loaded, packages);
        assert!(load_from(&path, BackendKind::Apt).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_old_or_foreign_header() {
        let dir = temp_dir("header");
        let path = dir.join("source.catalog");
        std::fs::write(&path, "firefox\tFirefox\t1\tBrowser\t\n").unwrap();
        assert!(load_from(&path, BackendKind::Pacman).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

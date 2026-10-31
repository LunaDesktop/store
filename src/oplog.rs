//! Dziennik operacji sklepu — wspólny dla wątku roboczego i strony „Status”.
//!
//! Każda instalacja/aktualizacja dopisuje wyjście menedżera pakietów do
//! `$XDG_CACHE_HOME/luna/store/operations.log`. Dzięki temu:
//! - strona „Status” może pokazywać log **na żywo**, gdy operacja trwa,
//! - po zamknięciu okna proces `pkexec`/menedżera nadal pisze do pliku,
//! - po ponownym uruchomieniu sklepu widać, co się wcześniej wydarzyło.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::cache;

/// Ścieżka dziennika operacji (obok cache katalogów).
pub fn log_path() -> Option<PathBuf> {
    cache::cache_dir().map(|dir| dir.join("operations.log"))
}

fn pid_path() -> Option<PathBuf> {
    cache::cache_dir().map(|dir| dir.join("operation.pid"))
}

/// Zapisuje PID procesu aktualizacji, żeby po restarcie UI widzieć, że trwa.
pub fn write_pid(pid: u32) {
    let Some(path) = pid_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(path, pid.to_string());
}

/// Usuwa plik PID (tylko gdy należy do wskazanego procesu).
pub fn clear_pid(pid: u32) {
    if running_pid() == Some(pid) {
        if let Some(path) = pid_path() {
            let _ = fs::remove_file(path);
        }
    }
}

/// PID trwającego procesu aktualizacji (jeśli nadal żyje).
pub fn running_pid() -> Option<u32> {
    let path = pid_path()?;
    let pid: u32 = fs::read_to_string(path).ok()?.trim().parse().ok()?;
    if pid == 0 {
        return None;
    }
    // `/proc/<pid>` znika razem z procesem; wystarczy do sprawdzenia „czy żyje”.
    Path::new(&format!("/proc/{pid}")).exists().then_some(pid)
}

/// Dopisuje jedną linię do dziennika (tworzy katalog/plik w razie potrzeby).
pub fn append(line: &str) {
    let Some(path) = log_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(file, "{line}");
    }
}

/// Ostatnie `max` linii dziennika.
pub fn tail(max: usize) -> Vec<String> {
    let Some(path) = log_path() else {
        return Vec::new();
    };
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    tail_from(&text, max)
}

fn tail_from(text: &str, max: usize) -> Vec<String> {
    let mut lines: Vec<String> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(str::to_owned)
        .collect();
    if lines.len() > max {
        lines = lines.split_off(lines.len() - max);
    }
    lines
}

/// Czyści dziennik (używane przez stronę „Status”).
pub fn clear() {
    if let Some(path) = log_path() {
        let _ = fs::write(path, "");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_limits_lines_and_skips_blanks() {
        // Test nie dotyka prawdziwego cache — sprawdza logikę na tekście.
        assert_eq!(tail_from("a\n\nb\nc\n", 2), vec!["b", "c"]);
        assert!(tail_from("", 5).is_empty());
        assert!(log_path().is_some());
    }
}

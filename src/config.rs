//! Konfiguracja sklepu wczytywana z pliku.
//!
//! Format jest celowo prosty (INI/TOML-podobny): sekcje `[store]`,
//! `[repository.<id>]` i pary `klucz = wartość`. Dzięki temu plik da się
//! edytować ręcznie i nie ciągniemy do aplikacji parsera TOML.
//!
//! Najważniejsza zasada: **lista repozytoriów pochodzi z pliku**, a nie z kodu.
//! Wbudowane wartości są tylko fallbackiem, gdy żadnego pliku nie ma.

use std::path::{Path, PathBuf};

use crate::model::BackendKind;

/// Wbudowana konfiguracja — identyczna z `data/repositories.conf`.
pub const DEFAULT_CONFIG: &str = include_str!("../data/repositories.conf");

/// Czy repozytorium ma być włączone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Enabled {
    /// Włącz, gdy menedżer pakietów jest dostępny.
    Auto,
    Yes,
    No,
}

impl Enabled {
    fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "yes" | "true" | "1" | "on" | "tak" => Self::Yes,
            "no" | "false" | "0" | "off" | "nie" => Self::No,
            _ => Self::Auto,
        }
    }
}

/// Sposób podnoszenia uprawnień przy instalacji/usuwaniu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Privilege {
    Auto,
    Pkexec,
    Sudo,
    None,
}

impl Privilege {
    fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "pkexec" | "polkit" => Self::Pkexec,
            "sudo" => Self::Sudo,
            "none" | "brak" | "no" => Self::None,
            _ => Self::Auto,
        }
    }
}

/// Opcjonalne nadpisania poleceń backendu.
///
/// Puste pole = użyj wbudowanej implementacji. Nadpisania dostają dane w formacie
/// TSV (szczegóły w `docs/store.md`), więc każdy system można podłączyć bez
/// rekompilacji sklepu.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Commands {
    pub catalog: Option<String>,
    pub installed: Option<String>,
    pub updates: Option<String>,
    pub install: Option<String>,
    pub remove: Option<String>,
    pub update: Option<String>,
    pub update_all: Option<String>,
}

/// Jedno źródło pakietów.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoConfig {
    pub id: String,
    pub name: String,
    pub backend: BackendKind,
    pub enabled: Enabled,
    /// Repozytorium Flatpaka (np. `flathub`).
    pub remote: Option<String>,
    /// `system` albo `user` — wymusza instalację Flatpaka.
    pub installation: Option<String>,
    /// Program sprawdzany przy `enabled = auto` (zamiast domyślnego).
    pub probe: Option<String>,
    pub overrides: Commands,
}

impl RepoConfig {
    pub fn is_active(&self) -> bool {
        match self.enabled {
            Enabled::Yes => true,
            Enabled::No => false,
            Enabled::Auto => {
                let probe = self
                    .probe
                    .as_deref()
                    .filter(|p| !p.is_empty())
                    .unwrap_or_else(|| self.backend.executable());
                !probe.is_empty() && which(probe).is_some()
            }
        }
    }
}

/// Cała konfiguracja sklepu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreConfig {
    pub refresh_minutes: u64,
    pub privilege: Privilege,
    /// Tokeny polecanych aplikacji (kolejność = kolejność na stronie „Odkrywaj”).
    pub featured: Vec<String>,
    pub repos: Vec<RepoConfig>,
    /// Plik, z którego wczytano konfigurację (do diagnostyki/UI).
    pub path: Option<PathBuf>,
    pub warning: Option<String>,
}

impl Default for StoreConfig {
    fn default() -> Self {
        Self::parse(DEFAULT_CONFIG)
    }
}

impl StoreConfig {
    /// Wczytuje konfigurację z pliku, a gdy go brak — zapisuje domyślną kopię
    /// w katalogu użytkownika i używa jej.
    pub fn load() -> Self {
        if let Some(path) = std::env::var_os("LUNA_STORE_CONFIG")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
        {
            return match std::fs::read_to_string(&path) {
                Ok(text) => {
                    let mut config = Self::parse(&text);
                    config.path = Some(path);
                    config
                }
                Err(err) => Self {
                    warning: Some(format!("{}: {err}", path.display())),
                    ..Self::default()
                },
            };
        }

        let mut candidates = Vec::new();
        if let Some(path) = user_config_path() {
            candidates.push(path);
        }
        candidates.push(PathBuf::from("/etc/luna/store/repositories.conf"));
        for dir in data_dirs() {
            candidates.push(dir.join("luna/store/repositories.conf"));
        }

        for path in candidates {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let mut config = Self::parse(&text);
            if config.repos.is_empty() {
                continue;
            }
            config.path = Some(path);
            return config;
        }

        Self {
            path: write_default_config(),
            ..Self::default()
        }
    }

    pub fn parse(src: &str) -> Self {
        let mut config = Self {
            refresh_minutes: 60,
            privilege: Privilege::Auto,
            featured: Vec::new(),
            repos: Vec::new(),
            path: None,
            warning: None,
        };
        let mut section: Option<String> = None;
        let mut current: Option<(String, Vec<(String, String)>)> = None;
        let mut blocks: Vec<(String, Vec<(String, String)>)> = Vec::new();

        for raw in src.lines() {
            let line = raw.trim().trim_start_matches('\u{feff}');
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') {
                if let Some(block) = current.take() {
                    blocks.push(block);
                }
                let name = line[1..line.len() - 1].trim().to_ascii_lowercase();
                section = Some(name.clone());
                current = name
                    .strip_prefix("repository.")
                    .map(|id| (id.trim().to_owned(), Vec::new()));
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim().to_ascii_lowercase();
            let value = value.trim();
            match section.as_deref() {
                Some("store") => match key.as_str() {
                    "refresh_minutes" | "refresh" => {
                        if let Ok(minutes) = value.parse::<u64>() {
                            config.refresh_minutes = minutes.max(1);
                        }
                    }
                    "privilege" => config.privilege = Privilege::parse(value),
                    "featured" | "recommended" => {
                        config.featured = split_list(value);
                    }
                    _ => {}
                },
                Some("featured") => {
                    if matches!(key.as_str(), "items" | "packages" | "apps") {
                        config.featured = split_list(value);
                    }
                }
                Some(_) => {
                    // Ten wariant obsługuje wyłącznie `[repository.*]`.
                    if let Some(block) = current.as_mut() {
                        block.1.push((key, value.to_owned()));
                    }
                }
                None => {}
            }
        }
        if let Some(block) = current.take() {
            blocks.push(block);
        }

        for (id, fields) in blocks {
            if let Some(repo) = build_repo(id, fields) {
                // Ostatnia definicja tego samego id wygrywa (jak w większości plików conf).
                config.repos.retain(|r| r.id != repo.id);
                config.repos.push(repo);
            }
        }
        config
    }

    /// Aktywne źródła w kolejności z pliku.
    pub fn active_repos(&self) -> Vec<RepoConfig> {
        self.repos
            .iter()
            .filter(|repo| repo.is_active())
            .cloned()
            .collect()
    }
}

fn build_repo(id: String, fields: Vec<(String, String)>) -> Option<RepoConfig> {
    let mut backend = None;
    let mut name = id.clone();
    let mut enabled = Enabled::Auto;
    let mut remote = None;
    let mut installation = None;
    let mut probe = None;
    let mut overrides = Commands::default();

    for (key, value) in fields {
        match key.as_str() {
            "backend" | "type" => backend = BackendKind::parse(&value),
            "name" | "label" => {
                if !value.is_empty() {
                    name = value;
                }
            }
            "enabled" => enabled = Enabled::parse(&value),
            "remote" => remote = non_empty(value),
            "installation" | "install_scope" => installation = non_empty(value),
            "probe" | "detect" => probe = non_empty(value),
            "catalog_cmd" | "list_cmd" | "search_cmd" => overrides.catalog = non_empty(value),
            "installed_cmd" => overrides.installed = non_empty(value),
            "updates_cmd" => overrides.updates = non_empty(value),
            "install_cmd" => overrides.install = non_empty(value),
            "remove_cmd" => overrides.remove = non_empty(value),
            "update_cmd" => overrides.update = non_empty(value),
            "update_all_cmd" | "upgrade_cmd" => overrides.update_all = non_empty(value),
            _ => {}
        }
    }

    let backend = backend?;
    Some(RepoConfig {
        id,
        name,
        backend,
        enabled,
        remote,
        installation,
        probe,
        overrides,
    })
}

fn non_empty(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value.trim().to_owned())
}

fn split_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|item| item.trim().to_lowercase())
        .filter(|item| !item.is_empty())
        .collect()
}

pub fn user_config_path() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(dir).join("luna/store/repositories.conf"));
    }
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".config/luna/store/repositories.conf"))
}

fn data_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    // XDG: dane użytkownika (`~/.local/share`) są przed katalogami systemowymi.
    if let Some(home) = std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
        dirs.push(PathBuf::from(home));
    } else if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join(".local/share"));
    }
    if let Some(system) = std::env::var_os("XDG_DATA_DIRS").filter(|v| !v.is_empty()) {
        dirs.extend(std::env::split_paths(&system));
    } else {
        dirs.push(PathBuf::from("/usr/local/share"));
        dirs.push(PathBuf::from("/usr/share"));
    }
    dirs
}

fn write_default_config() -> Option<PathBuf> {
    let path = user_config_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok()?;
    }
    std::fs::write(&path, DEFAULT_CONFIG).ok()?;
    Some(path)
}

/// Znajduje program w `PATH` (albo sprawdza ścieżkę bezwzględną).
pub fn which(program: &str) -> Option<PathBuf> {
    let program = program.trim();
    if program.is_empty() {
        return None;
    }
    let path = Path::new(program);
    if path.components().count() > 1 {
        return is_executable(path).then(|| path.to_path_buf());
    }
    if let Some(dirs) = std::env::var_os("PATH") {
        if let Some(found) = std::env::split_paths(&dirs)
            .map(|dir| dir.join(program))
            .find(|candidate| is_executable(candidate))
        {
            return Some(found);
        }
    }
    // Menedżery/`pkexec` bywają w `sbin`, a aplikacje z sesji graficznej dostają
    // czasem zawężony PATH — sprawdzamy więc także standardowe katalogi systemowe.
    const FALLBACK_DIRS: [&str; 6] = [
        "/usr/local/sbin",
        "/usr/local/bin",
        "/usr/sbin",
        "/usr/bin",
        "/sbin",
        "/bin",
    ];
    FALLBACK_DIRS
        .iter()
        .map(|dir| Path::new(dir).join(program))
        .find(|candidate| is_executable(candidate))
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
# komentarz
[store]
refresh_minutes = 15
privilege = pkexec
featured = firefox, gimp ,, vlc

[repository.test]
name = Test OS
backend = pacman
enabled = yes

[repository.custom]
name = Own
backend = custom
probe = /bin/sh
enabled = auto
catalog_cmd = sh -c 'echo hi'
install_cmd = pkexec own install {package}
remove_cmd = pkexec own remove {package}
update_all_cmd = pkexec own upgrade
"#;

    #[test]
    fn parses_store_and_repositories() {
        let config = StoreConfig::parse(SAMPLE);
        assert_eq!(config.refresh_minutes, 15);
        assert_eq!(config.privilege, Privilege::Pkexec);
        assert_eq!(config.featured, vec!["firefox", "gimp", "vlc"]);
        assert_eq!(config.repos.len(), 2);
        let test = &config.repos[0];
        assert_eq!(test.id, "test");
        assert_eq!(test.name, "Test OS");
        assert_eq!(test.backend, BackendKind::Pacman);
        assert_eq!(test.enabled, Enabled::Yes);
    }

    #[test]
    fn parses_custom_command_overrides() {
        let config = StoreConfig::parse(SAMPLE);
        let custom = config.repos.iter().find(|r| r.id == "custom").unwrap();
        assert_eq!(custom.backend, BackendKind::Custom);
        assert_eq!(custom.probe.as_deref(), Some("/bin/sh"));
        assert_eq!(custom.overrides.catalog.as_deref(), Some("sh -c 'echo hi'"));
        assert_eq!(
            custom.overrides.install.as_deref(),
            Some("pkexec own install {package}")
        );
    }

    #[test]
    fn builtin_defaults_cover_all_major_backends() {
        let config = StoreConfig::default();
        for backend in [
            BackendKind::Pacman,
            BackendKind::Pag,
            BackendKind::Apt,
            BackendKind::Dnf,
            BackendKind::Emerge,
            BackendKind::Flatpak,
        ] {
            assert!(
                config.repos.iter().any(|r| r.backend == backend),
                "brak backendu {backend}"
            );
        }
    }

    #[test]
    fn enabled_modes_and_probe_work() {
        let mut repo = RepoConfig {
            id: "x".into(),
            name: "X".into(),
            backend: BackendKind::Custom,
            enabled: Enabled::Auto,
            remote: None,
            installation: None,
            probe: None,
            overrides: Commands::default(),
        };
        assert!(!repo.is_active());
        repo.probe = Some("/bin/sh".into());
        assert!(repo.is_active());
        repo.enabled = Enabled::No;
        assert!(!repo.is_active());
        repo.enabled = Enabled::Yes;
        assert!(repo.is_active());
    }

    #[test]
    fn which_supports_absolute_and_path_lookup() {
        assert!(which("/bin/sh").is_some());
        assert!(which("sh").is_some());
        assert!(which("na-pewno-nie-ma-takiego-programu-xyz").is_none());
    }

    #[test]
    fn parse_keeps_last_definition_of_same_id() {
        let config = StoreConfig::parse(
            "[repository.a]\nbackend = pacman\nenabled = no\n\
             [repository.a]\nname = Drugi\nbackend = apt\nenabled = no\n",
        );
        assert_eq!(config.repos.len(), 1);
        assert_eq!(config.repos[0].name, "Drugi");
        assert_eq!(config.repos[0].backend, BackendKind::Apt);
    }
}

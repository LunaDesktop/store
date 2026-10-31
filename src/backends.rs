//! Backendy menedżerów pakietów.
//!
//! Każdy backend potrafi:
//! - pobrać katalog dostępnych pakietów,
//! - odczytać stan instalacji i dostępne aktualizacje,
//! - zbudować polecenia instalacji/usuwania/aktualizacji.
//!
//! Polecenia są uruchamiane **bez powłoki** (`Command::new` + argumenty), więc
//! nazwy pakietów nie mogą wstrzyknąć dodatkowych opcji. Wyjątkiem jest jawne
//! `sh -c '…'` w konfiguracji `backend = custom` — to plik zaufany przez
//! administratora/użytkownika.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;

use crate::config::{which, Privilege, RepoConfig};
use crate::model::{compare_versions, BackendKind, RawPackage};

/// Ustawienia przekazywane do wątku roboczego.
#[derive(Debug, Clone, Copy)]
pub struct WorkerSettings {
    pub privilege: Privilege,
}

impl WorkerSettings {
    pub fn new(privilege: Privilege) -> Self {
        Self { privilege }
    }
}

/// Stan źródła: zainstalowane wersje i dostępne aktualizacje.
#[derive(Debug, Clone, Default)]
pub struct Status {
    pub installed: Vec<(String, String)>,
    pub updates: Vec<(String, String)>,
    /// Niekrytyczny błąd (np. nie udało się pobrać listy aktualizacji).
    pub warning: Option<String>,
}

/// Co ma zrobić operacja.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpKind {
    Install,
    Remove,
    Update,
    UpdateAll,
}

/// Pojedyncze polecenie do wykonania.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Operation {
    pub program: String,
    pub args: Vec<String>,
    /// Czy dopisać `pkexec`/`sudo` według konfiguracji.
    pub privileged: bool,
}

struct Output {
    ok: bool,
    stdout: String,
    stderr: String,
    code: Option<i32>,
}

fn run_program(program: &str, args: &[String]) -> Result<Output, String> {
    let mut command = Command::new(program);
    command
        .args(args)
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .env("DEBIAN_FRONTEND", "noninteractive")
        .env("PAG_LANG", "en")
        .stdin(Stdio::null());
    let output = command
        .output()
        .map_err(|err| format!("{program}: {err}"))?;
    Ok(Output {
        ok: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        code: output.status.code(),
    })
}

fn query(program: &str, args: &[String]) -> Result<String, String> {
    let output = run_program(program, args)?;
    if output.ok {
        Ok(output.stdout)
    } else {
        Err(failure(program, &output))
    }
}

/// Jak `query`, ale brak aktualizacji (kod != 0 bez komunikatu) nie jest błędem.
fn query_optional(program: &str, args: &[String]) -> Result<String, String> {
    let output = run_program(program, args)?;
    if output.ok || (output.stdout.trim().is_empty() && output.stderr.trim().is_empty()) {
        Ok(output.stdout)
    } else {
        Err(failure(program, &output))
    }
}

fn failure(program: &str, output: &Output) -> String {
    let text = if output.stderr.trim().is_empty() {
        &output.stdout
    } else {
        &output.stderr
    };
    let tail = tail_lines(text, 6);
    let code = output
        .code
        .map(|code| format!(" (kod {code})"))
        .unwrap_or_default();
    if tail.is_empty() {
        format!("{program}{code}")
    } else {
        format!("{program}{code}: {tail}")
    }
}

fn tail_lines(text: &str, max: usize) -> String {
    let mut lines: Vec<String> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let mut line = line.trim().to_owned();
            if line.chars().count() > 200 {
                line = line.chars().take(200).collect::<String>() + "…";
            }
            line
        })
        .collect();
    if lines.len() > max {
        lines = lines.split_off(lines.len() - max);
    }
    lines.join("\n")
}

/// Wykonuje polecenia po kolei. Zwraca skrócony log ostatniego polecenia.
pub fn execute(settings: WorkerSettings, operations: &[Operation]) -> Result<String, String> {
    let mut log = String::new();
    for operation in operations {
        let (program, args) = apply_privilege(settings, operation)?;
        let output = run_program(&program, &args)?;
        if !output.ok {
            return Err(failure(&program, &output));
        }
        log = tail_lines(&output.stdout, 10);
    }
    Ok(log)
}

/// Ścieżka narzędzia do podnoszenia uprawnień (PKEXEC/SUDO) niezależnie od PATH.
fn privilege_program(name: &str) -> Option<String> {
    which(name).map(|path| path.display().to_string())
}

fn apply_privilege(
    settings: WorkerSettings,
    operation: &Operation,
) -> Result<(String, Vec<String>), String> {
    if !operation.privileged {
        return Ok((operation.program.clone(), operation.args.clone()));
    }
    let resolved = which(&operation.program)
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| operation.program.clone());
    let wrap = |program: String, prefix: &[&str]| {
        (
            program,
            prefix
                .iter()
                .map(|arg| (*arg).to_owned())
                .chain(std::iter::once(resolved.clone()))
                .chain(operation.args.clone())
                .collect(),
        )
    };
    match settings.privilege {
        Privilege::None => Ok((resolved, operation.args.clone())),
        Privilege::Pkexec => match privilege_program("pkexec") {
            Some(pkexec) => Ok(wrap(pkexec, &[])),
            None => Err(missing_authorization()),
        },
        Privilege::Sudo => match privilege_program("sudo") {
            Some(sudo) => Ok(wrap(sudo, &["-n"])),
            None => Err(missing_authorization()),
        },
        // Auto: preferujemy pkexec, bo tylko on pokazuje okno autoryzacji Polkit
        // (w Lunie rysuje je `luna-polkit`). Nie uruchamiamy operacji bez uprawnień
        // po cichu — lepiej pokazać jasny błąd w UI.
        Privilege::Auto => match privilege_program("pkexec") {
            Some(pkexec) => Ok(wrap(pkexec, &[])),
            None => Err(missing_authorization()),
        },
    }
}

fn missing_authorization() -> String {
    libluna_i18n::tr(
        "Administrator authorization is required to change packages (install polkit/pkexec)",
    )
}

// ── Katalog ──────────────────────────────────────────────────────────────────

pub fn catalog(repo: &RepoConfig) -> Result<Vec<RawPackage>, String> {
    if let Some(command) = &repo.overrides.catalog {
        return Ok(parse_tsv_catalog(&run_override(command)?));
    }
    let packages = match repo.backend {
        BackendKind::Pacman => pacman_catalog(),
        BackendKind::Pag => pag_catalog(),
        BackendKind::Apt => apt_catalog(),
        BackendKind::Dnf => dnf_catalog(),
        BackendKind::Emerge => emerge_catalog(),
        BackendKind::Flatpak => flatpak_catalog(repo),
        BackendKind::Custom => {
            return Err("backend = custom wymaga catalog_cmd w konfiguracji".to_owned())
        }
    }?;
    Ok(dedup(packages))
}

fn pacman_catalog() -> Result<Vec<RawPackage>, String> {
    Ok(parse_pacman_sl(&query("pacman", &args(&["-Sl"]))?))
}

fn apt_catalog() -> Result<Vec<RawPackage>, String> {
    Ok(parse_apt_search(&query(
        "apt-cache",
        &args(&["search", "."]),
    )?))
}

fn dnf_catalog() -> Result<Vec<RawPackage>, String> {
    Ok(
        parse_dnf_list(&query("dnf", &args(&["-q", "list", "--available"]))?)
            .into_iter()
            .map(|(id, version)| RawPackage {
                name: id.clone(),
                id,
                version,
                description: String::new(),
                remote: None,
            })
            .collect(),
    )
}

fn emerge_catalog() -> Result<Vec<RawPackage>, String> {
    let mut packages = Vec::new();
    let mut found = false;

    let mut roots = Vec::new();
    if let Ok(repos) = std::fs::read_dir("/var/db/repos") {
        for repo in repos.flatten() {
            roots.push(repo.path().join("metadata/md5-cache"));
        }
    }
    roots.push(PathBuf::from("/var/cache/edb/md5-cache"));

    for root in roots {
        if !root.is_dir() {
            continue;
        }
        found = true;
        let Ok(categories) = std::fs::read_dir(&root) else {
            continue;
        };
        for category in categories.flatten() {
            if !category.path().is_dir() {
                continue;
            }
            let category_name = category.file_name().to_string_lossy().to_string();
            let Ok(atoms) = std::fs::read_dir(category.path()) else {
                continue;
            };
            for atom in atoms.flatten() {
                if !atom.path().is_file() {
                    continue;
                }
                let name = atom.file_name().to_string_lossy().to_string();
                let (package, version) = split_name_version(&name);
                if package.is_empty() || version.is_empty() {
                    continue;
                }
                packages.push(RawPackage {
                    id: format!("{category_name}/{package}"),
                    name: package,
                    version,
                    description: String::new(),
                    remote: None,
                });
            }
        }
    }

    if !found {
        return Err(
            "emerge: brak /var/db/repos/*/metadata/md5-cache (uruchom emerge --sync)".to_owned(),
        );
    }
    Ok(packages)
}

fn pag_catalog() -> Result<Vec<RawPackage>, String> {
    match pag_catalog_json() {
        Ok(packages) if !packages.is_empty() => Ok(packages),
        _ => Ok(parse_pag_list(&query("pag", &args(&["list"]))?)),
    }
}

fn flatpak_catalog(repo: &RepoConfig) -> Result<Vec<RawPackage>, String> {
    let mut command = vec![
        "remote-ls".to_owned(),
        "--app".to_owned(),
        "--columns=application,name,version,description,origin".to_owned(),
    ];
    command.extend(flatpak_scope(repo));
    // Nie podajemy nazwy repozytorium jako argumentu pozycyjnego: gdy remote
    // istnieje w instalacji systemowej i użytkownika, Flatpak zgłasza błąd
    // „No remote chosen”. Pobieramy wszystkie i filtrujemy po kolumnie origin.
    let output = query("flatpak", &command)?;
    Ok(parse_flatpak_catalog(&output, repo.remote.as_deref()))
}

// ── Stan instalacji / aktualizacje ───────────────────────────────────────────

pub fn status(repo: &RepoConfig) -> Result<Status, String> {
    if repo.backend == BackendKind::Custom {
        let installed = match &repo.overrides.installed {
            Some(command) => parse_tsv_pairs(&run_override(command)?),
            None => return Err("backend = custom wymaga installed_cmd w konfiguracji".to_owned()),
        };
        let (updates, warning) = match &repo.overrides.updates {
            Some(command) => match run_override(command) {
                Ok(output) => (parse_tsv_updates(&output), None),
                Err(err) => (Vec::new(), Some(err)),
            },
            None => (Vec::new(), None),
        };
        return Ok(Status {
            installed: dedup_pairs(installed),
            updates,
            warning,
        });
    }

    match repo.backend {
        BackendKind::Pacman => status_with(repo, pacman_installed, pacman_updates),
        BackendKind::Pag => match (&repo.overrides.installed, &repo.overrides.updates) {
            (None, None) => pag_status(),
            _ => status_with(repo, pag_installed_cli, pag_updates_unavailable),
        },
        BackendKind::Apt => status_with(repo, apt_installed, apt_updates),
        BackendKind::Dnf => status_with(repo, dnf_installed, dnf_updates),
        BackendKind::Emerge => status_with(repo, emerge_installed, emerge_updates),
        BackendKind::Flatpak => {
            status_with(repo, || flatpak_installed(repo), || flatpak_updates(repo))
        }
        BackendKind::Custom => unreachable!(),
    }
}

fn status_with(
    repo: &RepoConfig,
    installed_default: impl FnOnce() -> Result<Vec<(String, String)>, String>,
    updates_default: impl FnOnce() -> Result<Vec<(String, String)>, String>,
) -> Result<Status, String> {
    let installed = match &repo.overrides.installed {
        Some(command) => parse_tsv_pairs(&run_override(command)?),
        None => installed_default()?,
    };
    let (updates, warning) = match &repo.overrides.updates {
        Some(command) => match run_override(command) {
            Ok(output) => (parse_tsv_updates(&output), None),
            Err(err) => (Vec::new(), Some(err)),
        },
        None => match updates_default() {
            Ok(updates) => (updates, None),
            Err(err) => (Vec::new(), Some(err)),
        },
    };
    Ok(Status {
        installed: dedup_pairs(installed),
        updates: dedup_pairs(updates),
        warning,
    })
}

fn pacman_installed() -> Result<Vec<(String, String)>, String> {
    Ok(parse_pairs(&query("pacman", &args(&["-Q"]))?))
}

fn pacman_updates() -> Result<Vec<(String, String)>, String> {
    Ok(parse_pacman_updates(&query_optional(
        "pacman",
        &args(&["-Qu"]),
    )?))
}

fn apt_installed() -> Result<Vec<(String, String)>, String> {
    Ok(parse_tsv_pairs(&query(
        "dpkg-query",
        &args(&["-W", "-f=${Package}\t${Version}\n"]),
    )?))
}

fn apt_updates() -> Result<Vec<(String, String)>, String> {
    Ok(parse_apt_upgradable(&query_optional(
        "apt",
        &args(&["list", "--upgradable", "-o", "APT::Color=0"]),
    )?))
}

fn dnf_installed() -> Result<Vec<(String, String)>, String> {
    Ok(parse_dnf_list(&query(
        "dnf",
        &args(&["-q", "list", "--installed"]),
    )?))
}

fn dnf_updates() -> Result<Vec<(String, String)>, String> {
    Ok(parse_dnf_list(&query_optional(
        "dnf",
        &args(&["-q", "list", "--upgrades"]),
    )?))
}

fn emerge_installed() -> Result<Vec<(String, String)>, String> {
    let mut packages = Vec::new();
    let root = Path::new("/var/db/pkg");
    let categories = std::fs::read_dir(root).map_err(|err| format!("{}: {err}", root.display()))?;
    for category in categories.flatten() {
        if !category.path().is_dir() {
            continue;
        }
        let category_name = category.file_name().to_string_lossy().to_string();
        let Ok(atoms) = std::fs::read_dir(category.path()) else {
            continue;
        };
        for atom in atoms.flatten() {
            if !atom.path().is_dir() {
                continue;
            }
            let name = atom.file_name().to_string_lossy().to_string();
            let (package, version) = split_name_version(&name);
            if package.is_empty() || version.is_empty() {
                continue;
            }
            packages.push((format!("{category_name}/{package}"), version));
        }
    }
    Ok(packages)
}

fn emerge_updates() -> Result<Vec<(String, String)>, String> {
    Ok(parse_emerge_updates(&query_optional(
        "emerge",
        &args(&["-puDN", "@world", "--quiet", "--color=n"]),
    )?))
}

fn emerge_describe(id: &str) -> Result<String, String> {
    let (category, package) = id
        .split_once('/')
        .ok_or_else(|| "Oczekiwano nazwy cat/pkg".to_owned())?;
    let mut roots = Vec::new();
    if let Ok(repos) = std::fs::read_dir("/var/db/repos") {
        for repo in repos.flatten() {
            roots.push(repo.path().join("metadata/md5-cache"));
        }
    }
    roots.push(PathBuf::from("/var/cache/edb/md5-cache"));
    let prefix = format!("{package}-");
    for root in roots {
        let dir = root.join(category);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with(&prefix) || !entry.path().is_file() {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(entry.path()) else {
                continue;
            };
            if let Some(description) = parse_md5_cache_description(&text) {
                return Ok(description);
            }
        }
    }
    Ok(String::new())
}

fn parse_md5_cache_description(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let description = clean(line.strip_prefix("DESCRIPTION=")?);
        (!description.is_empty()).then_some(description)
    })
}

fn flatpak_installed(repo: &RepoConfig) -> Result<Vec<(String, String)>, String> {
    let mut command = vec![
        "list".to_owned(),
        "--app".to_owned(),
        "--columns=application,version".to_owned(),
    ];
    command.extend(flatpak_scope(repo));
    Ok(parse_tsv_pairs(&query("flatpak", &command)?))
}

fn flatpak_updates(repo: &RepoConfig) -> Result<Vec<(String, String)>, String> {
    let mut command = vec![
        "remote-ls".to_owned(),
        "--updates".to_owned(),
        "--app".to_owned(),
        "--columns=application,version,origin".to_owned(),
    ];
    command.extend(flatpak_scope(repo));
    let output = query_optional("flatpak", &command)?;
    Ok(parse_flatpak_updates(&output, repo.remote.as_deref()))
}

fn flatpak_scope(repo: &RepoConfig) -> Vec<String> {
    let scope = repo
        .installation
        .as_deref()
        .unwrap_or("")
        .to_ascii_lowercase();
    match scope.as_str() {
        "user" => vec!["--user".to_owned()],
        "system" => vec!["--system".to_owned()],
        _ => Vec::new(),
    }
}

/// Wybiera instalację Flatpaka, w której istnieje dane repozytorium.
fn flatpak_install_scope(configured: &[String], remote: Option<&str>) -> Vec<String> {
    if !configured.is_empty() {
        return configured.to_vec();
    }
    let Some(remote) = remote else {
        return Vec::new();
    };
    for scope in ["--system", "--user"] {
        let command = vec![
            "remotes".to_owned(),
            scope.to_owned(),
            "--columns=name".to_owned(),
        ];
        if let Ok(output) = query("flatpak", &command) {
            if output.lines().any(|line| line.trim() == remote) {
                return vec![scope.to_owned()];
            }
        }
    }
    Vec::new()
}

// ── Pag: szybki odczyt JSON ──────────────────────────────────────────────────

const PAG_INSTALLED: &str = "/var/lib/pag/installed.json";
const PAG_PINNED: &str = "/var/lib/pag/pinned.json";
const PAG_REPOS: &str = "/var/cache/pag/repos";

#[derive(Debug, Clone)]
struct PagVersion {
    version: String,
    release: String,
}

fn pag_catalog_json() -> Result<Vec<RawPackage>, String> {
    let mut packages: HashMap<String, RawPackage> = HashMap::new();
    let mut found = false;
    let dir = Path::new(PAG_REPOS);
    let entries = std::fs::read_dir(dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(repo_packages) = parse_pag_repo_json(&text) else {
            continue;
        };
        found = true;
        for package in repo_packages {
            match packages.get(&package.id) {
                Some(existing)
                    if compare_versions(&existing.version, &package.version)
                        != std::cmp::Ordering::Less => {}
                _ => {
                    packages.insert(package.id.clone(), package);
                }
            }
        }
    }
    if !found {
        return Err(format!("brak indeksów pag w {}", dir.display()));
    }
    let mut out: Vec<RawPackage> = packages.into_values().collect();
    out.sort_by_key(|a| a.id.to_lowercase());
    Ok(out)
}

fn pag_status() -> Result<Status, String> {
    match pag_status_json() {
        Ok(status) => Ok(status),
        Err(json_error) => {
            let installed = pag_installed_cli()
                .map_err(|cli_error| format!("{json_error}; pag list: {cli_error}"))?;
            Ok(Status {
                installed,
                updates: Vec::new(),
                warning: Some(format!(
                    "{json_error} — używam CLI, aktualizacje mogą być niepełne"
                )),
            })
        }
    }
}

fn pag_status_json() -> Result<Status, String> {
    let installed_text =
        std::fs::read_to_string(PAG_INSTALLED).map_err(|err| format!("{PAG_INSTALLED}: {err}"))?;
    let installed = parse_pag_installed_json(&installed_text)?;

    let pinned = std::fs::read_to_string(PAG_PINNED)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| value.as_object().cloned())
        .map(|object| object.keys().cloned().collect::<HashSet<_>>())
        .unwrap_or_default();

    let (repo_versions, warning) = match pag_repo_versions_json() {
        Ok(versions) => (versions, None),
        Err(err) => (HashMap::new(), Some(err)),
    };

    let mut installed_out = Vec::with_capacity(installed.len());
    let mut updates = Vec::new();
    for item in installed {
        installed_out.push((
            item.0.clone(),
            pag_display(&item.1.version, &item.1.release),
        ));
        if pinned.contains(&item.0) {
            continue;
        }
        let Some(candidate) = repo_versions.get(&item.0) else {
            continue;
        };
        if pag_newer(candidate, &item.1) {
            updates.push((item.0, pag_display(&candidate.version, &candidate.release)));
        }
    }

    Ok(Status {
        installed: installed_out,
        updates,
        warning,
    })
}

fn pag_repo_versions_json() -> Result<HashMap<String, PagVersion>, String> {
    let mut versions: HashMap<String, PagVersion> = HashMap::new();
    let mut found = false;
    let dir = Path::new(PAG_REPOS);
    let entries = std::fs::read_dir(dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(packages) = parse_pag_versions_json(&text) else {
            continue;
        };
        found = true;
        for (name, version) in packages {
            match versions.get(&name) {
                Some(existing) if !pag_version_newer(&version, existing) => {}
                _ => {
                    versions.insert(name, version);
                }
            }
        }
    }
    if !found {
        return Err(format!("brak indeksów pag w {}", dir.display()));
    }
    Ok(versions)
}

fn pag_installed_cli() -> Result<Vec<(String, String)>, String> {
    Ok(
        parse_pag_list(&query("pag", &args(&["list", "--installed"]))?)
            .into_iter()
            .map(|package| (package.id, package.version))
            .collect(),
    )
}

fn pag_updates_unavailable() -> Result<Vec<(String, String)>, String> {
    Err("backend pag: aktualizacje wymagają czytelnego /var/lib/pag/installed.json".to_owned())
}

fn pag_newer(candidate: &PagVersion, current: &PagVersion) -> bool {
    match compare_versions(&candidate.version, &current.version) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Less => false,
        std::cmp::Ordering::Equal => {
            compare_versions(&candidate.release, &current.release) == std::cmp::Ordering::Greater
        }
    }
}

fn pag_version_newer(candidate: &PagVersion, current: &PagVersion) -> bool {
    pag_newer(candidate, current)
}

fn pag_display(version: &str, release: &str) -> String {
    if release.is_empty() || release == "1" {
        version.to_owned()
    } else {
        format!("{version}-{release}")
    }
}

fn parse_pag_repo_json(text: &str) -> Result<Vec<RawPackage>, String> {
    let value: Value = serde_json::from_str(text).map_err(|err| err.to_string())?;
    let packages = value
        .get("packages")
        .and_then(Value::as_array)
        .ok_or_else(|| "brak pola packages".to_owned())?;
    let mut out = Vec::with_capacity(packages.len());
    for package in packages {
        let name = package
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if name.is_empty() {
            continue;
        }
        let version = json_scalar(package.get("version"));
        let release = json_scalar(package.get("release"));
        let description = package
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("");
        out.push(RawPackage {
            id: name.to_owned(),
            name: name.to_owned(),
            version: pag_display(&version, &release),
            description: clean(description),
            remote: None,
        });
    }
    Ok(out)
}

fn parse_pag_versions_json(text: &str) -> Result<HashMap<String, PagVersion>, String> {
    let value: Value = serde_json::from_str(text).map_err(|err| err.to_string())?;
    let packages = value
        .get("packages")
        .and_then(Value::as_array)
        .ok_or_else(|| "brak pola packages".to_owned())?;
    let mut out = HashMap::new();
    for package in packages {
        let Some(name) = package.get("name").and_then(Value::as_str) else {
            continue;
        };
        out.insert(
            name.to_owned(),
            PagVersion {
                version: json_scalar(package.get("version")),
                release: json_scalar(package.get("release")),
            },
        );
    }
    Ok(out)
}

fn parse_pag_installed_json(text: &str) -> Result<Vec<(String, PagVersion)>, String> {
    let value: Value = serde_json::from_str(text).map_err(|err| err.to_string())?;
    let object = value
        .as_object()
        .ok_or_else(|| "oczekiwano obiektu JSON".to_owned())?;
    let mut out = Vec::with_capacity(object.len());
    for (name, info) in object {
        if !info.is_object() {
            continue;
        }
        out.push((
            name.clone(),
            PagVersion {
                version: json_scalar(info.get("version")),
                release: json_scalar(info.get("release")),
            },
        ));
    }
    Ok(out)
}

fn json_scalar(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(value)) => value.clone(),
        Some(Value::Number(value)) => value.to_string(),
        Some(Value::Bool(value)) => value.to_string(),
        _ => String::new(),
    }
}

// ── Opis pakietu (leniwe dociąganie) ─────────────────────────────────────────

pub fn describe(repo: &RepoConfig, id: &str) -> Result<String, String> {
    if !valid_package(id) {
        return Err("Nieprawidłowa nazwa pakietu".to_owned());
    }
    match repo.backend {
        BackendKind::Pacman => {
            let output = query("pacman", &args(&["-Si", id]))?;
            Ok(parse_pacman_info(&output))
        }
        BackendKind::Dnf => {
            let output = query("dnf", &args(&["-q", "info", id]))?;
            Ok(parse_dnf_info(&output))
        }
        BackendKind::Emerge => emerge_describe(id),
        // Pozostałe backendy dostarczają opis w katalogu.
        _ => Ok(String::new()),
    }
}

// ── Operacje ─────────────────────────────────────────────────────────────────

pub fn operations(
    repo: &RepoConfig,
    kind: OpKind,
    id: &str,
    remote: Option<&str>,
) -> Result<Vec<Operation>, String> {
    if kind != OpKind::UpdateAll && !valid_package(id) {
        return Err(format!("Nieprawidłowa nazwa pakietu: {id}"));
    }
    let command = match kind {
        OpKind::Install => &repo.overrides.install,
        OpKind::Remove => &repo.overrides.remove,
        OpKind::Update => &repo.overrides.update,
        OpKind::UpdateAll => &repo.overrides.update_all,
    };
    if let Some(command) = command {
        return Ok(vec![override_operation(command, id, remote)?]);
    }

    let ops = match repo.backend {
        BackendKind::Pacman => match kind {
            OpKind::Install | OpKind::Update => vec![op(
                "pacman",
                &["-S", "--noconfirm", "--needed", "--", id],
                true,
            )],
            OpKind::Remove => vec![op("pacman", &["-Rns", "--noconfirm", "--", id], true)],
            OpKind::UpdateAll => vec![op("pacman", &["-Syu", "--noconfirm"], true)],
        },
        BackendKind::Pag => match kind {
            OpKind::Install | OpKind::Update => vec![op("pag", &["install", "-y", id], true)],
            OpKind::Remove => vec![op("pag", &["remove", "-y", id], true)],
            OpKind::UpdateAll => vec![op("pag", &["update", "-y"], true)],
        },
        BackendKind::Apt => {
            // W GUI nie ma terminala do pytań dpkg — wybieramy bezpieczne
            // zachowanie starej konfiguracji.
            const CONFIG: [&str; 4] = [
                "-o",
                "Dpkg::Options::=--force-confdef",
                "-o",
                "Dpkg::Options::=--force-confold",
            ];
            match kind {
                OpKind::Install => vec![op(
                    "apt-get",
                    &[
                        "install", "-y", CONFIG[0], CONFIG[1], CONFIG[2], CONFIG[3], "--", id,
                    ],
                    true,
                )],
                OpKind::Remove => vec![op("apt-get", &["remove", "-y", "--", id], true)],
                OpKind::Update => vec![op(
                    "apt-get",
                    &[
                        "install",
                        "-y",
                        "--only-upgrade",
                        CONFIG[0],
                        CONFIG[1],
                        CONFIG[2],
                        CONFIG[3],
                        "--",
                        id,
                    ],
                    true,
                )],
                // Najpierw odśwież indeksy (inaczej lista aktualizacji może być
                // z poprzedniego uruchomienia systemu), potem aktualizuj.
                OpKind::UpdateAll => vec![
                    op("apt-get", &["update"], true),
                    op(
                        "apt-get",
                        &["upgrade", "-y", CONFIG[0], CONFIG[1], CONFIG[2], CONFIG[3]],
                        true,
                    ),
                ],
            }
        }
        BackendKind::Dnf => match kind {
            OpKind::Install => vec![op("dnf", &["install", "-y", "--", id], true)],
            OpKind::Remove => vec![op("dnf", &["remove", "-y", "--", id], true)],
            OpKind::Update => vec![op("dnf", &["upgrade", "-y", "--", id], true)],
            OpKind::UpdateAll => vec![op("dnf", &["upgrade", "-y"], true)],
        },
        BackendKind::Emerge => match kind {
            OpKind::Install => vec![op("emerge", &["--ask=n", "--quiet", id], true)],
            OpKind::Remove => vec![op("emerge", &["--unmerge", "--ask=n", id], true)],
            OpKind::Update => vec![op(
                "emerge",
                &["--update", "--deep", "--newuse", "--ask=n", id],
                true,
            )],
            OpKind::UpdateAll => vec![op(
                "emerge",
                &["--update", "--deep", "--newuse", "--ask=n", "@world"],
                true,
            )],
        },
        BackendKind::Flatpak => {
            let scope = flatpak_scope(repo);
            let selected_remote = remote.map(str::to_owned).or_else(|| repo.remote.clone());
            match kind {
                OpKind::Install => {
                    // Gdy remote istnieje w obu instalacjach, bez `--system`/
                    // `--user` Flatpak nie wie, którą wybrać. Sprawdzamy, gdzie
                    // remote jest zarejestrowany (system/user) i wybieramy to.
                    let install_scope = flatpak_install_scope(&scope, selected_remote.as_deref());
                    let mut command = vec![
                        "install".to_owned(),
                        "--noninteractive".to_owned(),
                        "-y".to_owned(),
                    ];
                    command.extend(install_scope);
                    command.extend(
                        selected_remote
                            .iter()
                            .cloned()
                            .chain(std::iter::once(id.to_owned())),
                    );
                    vec![Operation {
                        program: "flatpak".into(),
                        args: command,
                        privileged: false,
                    }]
                }
                OpKind::Remove => {
                    let mut command = vec![
                        "uninstall".to_owned(),
                        "--noninteractive".to_owned(),
                        "-y".to_owned(),
                    ];
                    command.extend(scope);
                    command.push(id.to_owned());
                    vec![Operation {
                        program: "flatpak".into(),
                        args: command,
                        privileged: false,
                    }]
                }
                OpKind::Update => {
                    let mut command = vec![
                        "update".to_owned(),
                        "--noninteractive".to_owned(),
                        "-y".to_owned(),
                    ];
                    command.extend(scope);
                    command.push(id.to_owned());
                    vec![Operation {
                        program: "flatpak".into(),
                        args: command,
                        privileged: false,
                    }]
                }
                OpKind::UpdateAll => {
                    let mut command = vec![
                        "update".to_owned(),
                        "--noninteractive".to_owned(),
                        "-y".to_owned(),
                    ];
                    command.extend(scope);
                    vec![Operation {
                        program: "flatpak".into(),
                        args: command,
                        privileged: false,
                    }]
                }
            }
        }
        BackendKind::Custom => {
            return Err("backend = custom wymaga odpowiedniego *_cmd w konfiguracji".to_owned())
        }
    };
    Ok(ops)
}

fn op(program: &str, args: &[&str], privileged: bool) -> Operation {
    Operation {
        program: program.to_owned(),
        args: args.iter().map(|arg| (*arg).to_owned()).collect(),
        privileged,
    }
}

fn override_operation(command: &str, id: &str, remote: Option<&str>) -> Result<Operation, String> {
    let argv = split_command(command)?;
    let program = argv[0].clone();
    let mut args = Vec::new();
    let mut used_package = false;
    for argument in &argv[1..] {
        let had_package = argument.contains("{package}");
        let had_remote = argument.contains("{remote}");
        used_package |= had_package;
        let value = argument
            .replace("{package}", id)
            .replace("{remote}", remote.unwrap_or(""));
        if value.is_empty() && had_remote && remote.is_none() {
            // `--remote {remote}` bez zdalnego repozytorium nie może zostawić
            // samej opcji bez wartości.
            if args
                .last()
                .is_some_and(|last| last == "--remote" || last == "-r")
            {
                args.pop();
            }
            continue;
        }
        args.push(value);
    }
    if !used_package && !id.is_empty() {
        args.push(id.to_owned());
    }
    Ok(Operation {
        program,
        args,
        privileged: false,
    })
}

/// Czy nazwa pakietu jest bezpieczna do przekazania menedżerowi.
pub fn valid_package(id: &str) -> bool {
    let id = id.trim();
    !id.is_empty()
        && id.len() <= 255
        && !id.starts_with('-')
        && id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || "@._+:-/~=[]<>".contains(ch))
}

fn run_override(command: &str) -> Result<String, String> {
    let argv = split_command(command)?;
    query(&argv[0], &argv[1..])
}

/// Dzieli polecenie z konfiguracji na argumenty, z obsługą cudzysłowów.
pub fn split_command(command: &str) -> Result<Vec<String>, String> {
    let mut arguments = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    for ch in command.chars() {
        match quote {
            Some(q) if ch == q => quote = None,
            Some(_) => current.push(ch),
            None if ch == '\'' || ch == '"' => quote = Some(ch),
            None if ch.is_whitespace() => {
                if !current.is_empty() {
                    arguments.push(std::mem::take(&mut current));
                }
            }
            None => current.push(ch),
        }
    }
    if quote.is_some() {
        return Err("Niedomknięty cudzysłów w poleceniu".to_owned());
    }
    if !current.is_empty() {
        arguments.push(current);
    }
    if arguments.is_empty() {
        return Err("Puste polecenie".to_owned());
    }
    Ok(arguments)
}

fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

// ── Parsery ──────────────────────────────────────────────────────────────────

fn clean(text: &str) -> String {
    let mut out = String::with_capacity(text.len().min(512));
    let mut last_space = false;
    for ch in text.chars() {
        let ch = if ch == '\t' || ch == '\r' || ch == '\n' {
            ' '
        } else {
            ch
        };
        if ch == ' ' {
            if last_space || out.is_empty() {
                continue;
            }
            last_space = true;
        } else {
            last_space = false;
        }
        out.push(ch);
        if out.chars().count() >= 600 {
            out.push('…');
            break;
        }
    }
    out.trim().to_owned()
}

fn dedup(packages: Vec<RawPackage>) -> Vec<RawPackage> {
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(packages.len());
    for mut package in packages {
        if package.id.is_empty() || !seen.insert(package.id.clone()) {
            continue;
        }
        package.name = clean(&package.name);
        package.description = clean(&package.description);
        package.version = clean(&package.version);
        out.push(package);
    }
    out
}

fn dedup_pairs(pairs: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut map = HashMap::new();
    for (id, version) in pairs {
        if !id.is_empty() {
            map.insert(id, version);
        }
    }
    let mut out: Vec<_> = map.into_iter().collect();
    out.sort_by_key(|a| a.0.to_lowercase());
    out
}

fn parse_pairs(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim_end_matches('\r').trim();
            if line.is_empty() {
                return None;
            }
            let mut fields = line.split_whitespace();
            let id = fields.next()?;
            let version = fields.next().unwrap_or("");
            Some((clean(id), clean(version)))
        })
        .collect()
}

fn parse_tsv_pairs(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = if line.contains('\t') {
            line.splitn(2, '\t').map(str::trim).collect::<Vec<_>>()
        } else {
            line.split_whitespace().take(2).collect::<Vec<_>>()
        };
        if fields.is_empty() {
            continue;
        }
        let id = fields.remove(0);
        let version = fields.first().copied().unwrap_or("");
        if !id.is_empty() {
            out.push((clean(id), clean(version)));
        }
    }
    out
}

fn parse_tsv_updates(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = if line.contains('\t') {
            line.split('\t').map(str::trim).collect()
        } else {
            line.split_whitespace().collect()
        };
        match fields.len() {
            0 | 1 => {}
            2 => out.push((clean(fields[0]), clean(fields[1]))),
            _ => out.push((clean(fields[0]), clean(fields[2]))),
        }
    }
    out
}

fn parse_tsv_catalog(text: &str) -> Vec<RawPackage> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let (id, name, version, description) = if line.contains('\t') {
            let mut fields = line.splitn(4, '\t').map(str::trim);
            let id = fields.next().unwrap_or("");
            let name = fields.next().unwrap_or("");
            let version = fields.next().unwrap_or("");
            let description = fields.next().unwrap_or("");
            (id, name, version, description)
        } else {
            let mut fields = line.split_whitespace();
            let id = fields.next().unwrap_or("");
            let name = fields.next().unwrap_or("");
            let version = fields.next().unwrap_or("");
            (id, name, version, "")
        };
        if id.is_empty() {
            continue;
        }
        out.push(RawPackage {
            id: clean(id),
            name: clean(if name.is_empty() { id } else { name }),
            version: clean(version),
            description: clean(description),
            remote: None,
        });
    }
    out
}

fn parse_pacman_sl(text: &str) -> Vec<RawPackage> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let _repository = fields.next()?;
            let name = fields.next()?;
            let version = fields.next()?;
            Some(RawPackage {
                id: clean(name),
                name: clean(name),
                version: clean(version),
                description: String::new(),
                remote: None,
            })
        })
        .collect()
}

fn parse_pacman_updates(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.is_empty() {
            continue;
        }
        let id = clean(fields[0]);
        if let Some(arrow) = fields.iter().position(|field| *field == "->") {
            if let Some(version) = fields.get(arrow + 1) {
                out.push((id, clean(version)));
            }
        } else if let Some(version) = fields.get(1) {
            out.push((id, clean(version)));
        }
    }
    out
}

fn parse_pacman_info(text: &str) -> String {
    for line in text.lines() {
        if let Some((key, value)) = line.split_once(':') {
            if key.trim().eq_ignore_ascii_case("description") {
                return clean(value);
            }
        }
    }
    String::new()
}

fn parse_apt_search(text: &str) -> Vec<RawPackage> {
    text.lines()
        .filter_map(|line| {
            let (name, description) = line.split_once(" - ")?;
            let name = name.trim();
            if name.is_empty() || name.contains(char::is_whitespace) {
                return None;
            }
            Some(RawPackage {
                id: clean(name),
                name: clean(name),
                version: String::new(),
                description: clean(description),
                remote: None,
            })
        })
        .collect()
}

fn parse_apt_upgradable(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty()
            || line.starts_with("Listing")
            || line.starts_with("WARNING")
            || line.starts_with("N:")
        {
            continue;
        }
        let mut fields = line.split_whitespace();
        let Some(first) = fields.next() else {
            continue;
        };
        let Some((name, _repository)) = first.split_once('/') else {
            continue;
        };
        let Some(version) = fields.next() else {
            continue;
        };
        if valid_package(name) {
            out.push((clean(name), clean(version)));
        }
    }
    out
}

fn parse_dnf_list(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty()
            || line.ends_with("Packages")
            || line.starts_with("Last metadata")
            || line.starts_with("Metadata")
        {
            continue;
        }
        let mut fields = line.split_whitespace();
        let Some(name) = fields.next() else {
            continue;
        };
        let Some(version) = fields.next() else {
            continue;
        };
        let name = strip_arch(name);
        if valid_package(&name) {
            out.push((clean(&name), clean(version)));
        }
    }
    out
}

fn strip_arch(name: &str) -> String {
    const ARCHES: &[&str] = &[
        "x86_64", "amd64", "aarch64", "arm64", "i686", "i386", "noarch", "all", "armv7hl",
        "armv6hl", "ppc64le", "ppc64", "s390x", "riscv64", "src",
    ];
    match name.rsplit_once('.') {
        Some((base, arch)) if ARCHES.contains(&arch) => base.to_owned(),
        _ => name.to_owned(),
    }
}

fn parse_dnf_info(text: &str) -> String {
    for line in text.lines() {
        if let Some((key, value)) = line.split_once(':') {
            let key = key.trim().to_ascii_lowercase();
            if key == "summary" || key == "opis" || key == "description" {
                let value = clean(value);
                if !value.is_empty() {
                    return value;
                }
            }
        }
    }
    String::new()
}

fn parse_emerge_updates(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if !line.contains('/') {
            continue;
        }
        let Some(rest) = line.split_once(']').map(|(_, rest)| rest.trim()) else {
            continue;
        };
        let mut fields = rest.split_whitespace();
        let Some(atom) = fields.next() else {
            continue;
        };
        if atom.starts_with('[') || atom.starts_with("world") {
            continue;
        }
        let atom = atom.split("::").next().unwrap_or(atom);
        let (category, name_version) = match atom.split_once('/') {
            Some(parts) => parts,
            None => continue,
        };
        // `cat/pkg-1.2:0` → `pkg-1.2` (slot nie jest częścią wersji).
        let name_version = name_version.split(':').next().unwrap_or(name_version);
        let (name, version) = split_name_version(name_version);
        if name.is_empty() || version.is_empty() {
            continue;
        }
        out.push((format!("{category}/{name}"), version));
    }
    out
}

fn parse_pag_list(text: &str) -> Vec<RawPackage> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty()
            || line.starts_with("Dostępne")
            || line.starts_with("Zainstalowane")
            || line.starts_with("Available")
            || line.starts_with("Installed")
            || line.starts_with("Wyniki")
            || line.starts_with("Results")
            || line.starts_with('❌')
        {
            continue;
        }
        let line = line
            .strip_prefix("[✓]")
            .or_else(|| line.strip_prefix("[ ]"))
            .unwrap_or(line)
            .trim();
        let (left, right) = line
            .split_once("  – ")
            .or_else(|| line.split_once(" - "))
            .unwrap_or((line, ""));
        let left = left.split("  [").next().unwrap_or(left).trim();
        let (name, version) = split_name_version(left);
        if name.is_empty() {
            continue;
        }
        let description = right.split("  [").next().unwrap_or(right);
        out.push(RawPackage {
            id: clean(&name),
            name: clean(&name),
            version: clean(&version),
            description: clean(description),
            remote: None,
        });
    }
    out
}

fn parse_flatpak_catalog(text: &str, remote_filter: Option<&str>) -> Vec<RawPackage> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() < 4 {
            continue;
        }
        let id = fields[0].trim();
        let name = fields[1].trim();
        let version = fields[2].trim();
        let description = fields[3].trim();
        let origin = fields.get(4).copied().unwrap_or("").trim();
        if id.is_empty() {
            continue;
        }
        if let Some(filter) = remote_filter {
            if !filter.is_empty() && !origin.is_empty() && origin != filter {
                continue;
            }
        }
        if !seen.insert(id.to_owned()) {
            continue;
        }
        out.push(RawPackage {
            id: clean(id),
            name: clean(if name.is_empty() { id } else { name }),
            version: clean(version),
            description: clean(description),
            remote: (!origin.is_empty()).then(|| clean(origin)),
        });
    }
    out
}

fn parse_flatpak_updates(text: &str, remote_filter: Option<&str>) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() < 2 {
            continue;
        }
        let id = fields[0].trim();
        let version = fields[1].trim();
        let origin = fields.get(2).copied().unwrap_or("").trim();
        if id.is_empty() {
            continue;
        }
        if let Some(filter) = remote_filter {
            if !filter.is_empty() && !origin.is_empty() && origin != filter {
                continue;
            }
        }
        out.push((clean(id), clean(version)));
    }
    out
}

/// Dzieli `nazwa-wersja` (np. `foo-bar-1.2.3-r1`) na nazwę i wersję.
///
/// Wersja zaczyna się od cyfry albo od `r`/`v` + cyfra. Szukamy od prawej,
/// więc nazwy z myślnikami działają poprawnie.
fn split_name_version(input: &str) -> (String, String) {
    // Najpierw szukamy wersji zaczynającej się od cyfry (np. `1.2.3-r1`).
    for (index, _) in input.match_indices('-').rev() {
        let suffix = &input[index + 1..];
        if suffix.starts_with(|ch: char| ch.is_ascii_digit()) {
            return (input[..index].to_owned(), suffix.to_owned());
        }
    }
    // Potem wariantów `r123` / `v1.2` (np. pag używa `r33.0.1`).
    for (index, _) in input.match_indices('-').rev() {
        let suffix = &input[index + 1..];
        let mut chars = suffix.chars();
        if let Some(first) = chars.next() {
            if (first == 'r' || first == 'v')
                && chars.next().is_some_and(|next| next.is_ascii_digit())
            {
                return (input[..index].to_owned(), suffix.to_owned());
            }
        }
    }
    (input.to_owned(), String::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Commands;
    use crate::model::BackendKind;

    fn repo(backend: BackendKind) -> RepoConfig {
        RepoConfig {
            id: "test".into(),
            name: "Test".into(),
            backend,
            enabled: crate::config::Enabled::Yes,
            remote: None,
            installation: None,
            probe: None,
            overrides: Commands::default(),
        }
    }

    #[test]
    fn parses_pacman_catalog_installed_and_updates() {
        let catalog =
            parse_pacman_sl("core bash 5.2.1-1\ncore zlib 1.3-1\nextra firefox 130.0-1\n");
        assert_eq!(catalog.len(), 3);
        assert_eq!(catalog[0].id, "bash");
        assert_eq!(catalog[2].version, "130.0-1");

        let installed = parse_pairs("bash 5.2.1-1\nzlib 1.3-1\n");
        assert_eq!(installed.len(), 2);
        assert_eq!(installed[1].0, "zlib");

        let updates =
            parse_pacman_updates("bash 5.2.1-1 -> 5.2.2-1\nzlib 1.3-1 -> 1.4-1 [ignored]\n");
        assert_eq!(updates[0], ("bash".to_owned(), "5.2.2-1".to_owned()));
        assert_eq!(updates[1].1, "1.4-1");
    }

    #[test]
    fn parses_apt_and_dnf() {
        let catalog =
            parse_apt_search("bash - GNU Bourne Again SHell\nzlib1g - compression library\n");
        assert_eq!(catalog.len(), 2);
        assert_eq!(catalog[0].id, "bash");
        assert_eq!(catalog[1].description, "compression library");

        let updates = parse_apt_upgradable(
            "Listing...\nbash/noble-updates 5.2.21-2ubuntu4 amd64 [upgradable from: 5.2.21-2ubuntu4]\n",
        );
        assert_eq!(
            updates,
            vec![("bash".to_owned(), "5.2.21-2ubuntu4".to_owned())]
        );

        let dnf = parse_dnf_list(
            "bash.x86_64 5.2.26-4.fc41 updates\npython3.noarch 3.13.0-1.fc41 fedora\n",
        );
        assert_eq!(dnf[0], ("bash".to_owned(), "5.2.26-4.fc41".to_owned()));
        assert_eq!(dnf[1].0, "python3");
    }

    #[test]
    fn parses_pag_cli_fallback() {
        let catalog = parse_pag_list(
            "Dostępne (2):\n  [✓] bash-5.3.15  – The Bash shell  [zainstalowany: 5.3.15]\n  [ ] foo-bar-1.2.3  – Example package\n",
        );
        assert_eq!(catalog.len(), 2);
        assert_eq!(catalog[0].id, "bash");
        assert_eq!(catalog[0].version, "5.3.15");
        assert_eq!(catalog[1].id, "foo-bar");
        assert_eq!(catalog[1].version, "1.2.3");
        assert_eq!(catalog[1].description, "Example package");
    }

    #[test]
    fn parses_pag_json() {
        let repo_json = r#"{
            "packages": [
                {"name": "bash", "version": "5.3.15", "release": "2", "description": "Shell"},
                {"name": "zlib", "version": "1.3", "release": 1, "description": "Compression"}
            ]
        }"#;
        let packages = parse_pag_repo_json(repo_json).unwrap();
        assert_eq!(packages.len(), 2);
        assert_eq!(packages[0].version, "5.3.15-2");
        assert_eq!(packages[1].version, "1.3");

        let installed_json = r#"{
            "bash": {"version": "5.3.15", "release": "1"},
            "zlib": {"version": "1.3", "release": "1"}
        }"#;
        let installed = parse_pag_installed_json(installed_json).unwrap();
        assert_eq!(installed.len(), 2);
        let versions = parse_pag_versions_json(repo_json).unwrap();
        assert!(pag_newer(
            &versions["bash"],
            &installed.iter().find(|p| p.0 == "bash").unwrap().1
        ));
        assert!(!pag_newer(
            &versions["zlib"],
            &installed.iter().find(|p| p.0 == "zlib").unwrap().1
        ));
    }

    #[test]
    fn parses_emerge_and_flatpak() {
        let updates = parse_emerge_updates(
            "[ebuild  U  ] sys-apps/foo-2.0 [1.0]\n[ebuild  U  ] dev-libs/bar-1.2.3-r1 [1.2.3]\n",
        );
        assert_eq!(updates[0], ("sys-apps/foo".to_owned(), "2.0".to_owned()));
        assert_eq!(
            updates[1],
            ("dev-libs/bar".to_owned(), "1.2.3-r1".to_owned())
        );

        let catalog = parse_flatpak_catalog(
            "org.mozilla.firefox\tFirefox\t130.0\tWeb browser\tflathub\norg.gimp.GIMP\tGIMP\t2.10\tImage editor\tflathub\n",
            Some("flathub"),
        );
        assert_eq!(catalog.len(), 2);
        assert_eq!(catalog[0].remote.as_deref(), Some("flathub"));

        let filtered = parse_flatpak_catalog(
            "org.mozilla.firefox\tFirefox\t130.0\tWeb browser\tflathub\norg.gnome.Builder\tBuilder\t46\tIDE\tgnome-nightly\n",
            Some("flathub"),
        );
        assert_eq!(filtered.len(), 1);
    }

    #[test]
    fn parses_gentoo_md5_cache_description() {
        let text = "DEFINED_PHASES=install\nDESCRIPTION=Example package for tests\nHOMEPAGE=https://example.org\n";
        assert_eq!(
            parse_md5_cache_description(text).as_deref(),
            Some("Example package for tests")
        );
        assert_eq!(parse_md5_cache_description("A=1\n"), None);
    }

    #[test]
    fn parses_custom_tsv_formats() {
        let catalog =
            parse_tsv_catalog("foo\tFoo\t1.2\tFirst app\nbar\tBar\t2.0\tSecond app, with comma\n");
        assert_eq!(catalog.len(), 2);
        assert_eq!(catalog[0].description, "First app");
        assert_eq!(catalog[1].id, "bar");

        let pairs = parse_tsv_pairs("foo\t1.2\nbar 2.0\n");
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[1], ("bar".to_owned(), "2.0".to_owned()));

        let updates = parse_tsv_updates("foo\t1.2\t1.3\nbar\t2.1\n");
        assert_eq!(updates[0], ("foo".to_owned(), "1.3".to_owned()));
        assert_eq!(updates[1], ("bar".to_owned(), "2.1".to_owned()));
    }

    #[test]
    fn splits_commands_with_quotes() {
        let argv = split_command("sh -c 'xbps-query -l | awk \"{print}\"'").unwrap();
        assert_eq!(argv[0], "sh");
        assert_eq!(argv[1], "-c");
        assert_eq!(argv[2], "xbps-query -l | awk \"{print}\"");
        assert!(split_command("sh -c 'niedomknięte").is_err());
    }

    #[test]
    fn validates_package_names() {
        assert!(valid_package("firefox"));
        assert!(valid_package("dev-libs/glib"));
        assert!(valid_package("org.gimp.GIMP"));
        assert!(valid_package("python3.12:amd64"));
        assert!(!valid_package("-rf"));
        assert!(!valid_package("foo bar"));
        assert!(!valid_package("foo;rm"));
        assert!(!valid_package(""));
    }

    #[test]
    fn builds_apt_operations_without_interactive_dpkg() {
        let ops = operations(&repo(BackendKind::Apt), OpKind::Install, "bash", None).unwrap();
        assert_eq!(ops[0].program, "apt-get");
        assert!(ops[0]
            .args
            .iter()
            .any(|arg| arg == "Dpkg::Options::=--force-confold"));
        let all = operations(&repo(BackendKind::Apt), OpKind::UpdateAll, "", None).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].args, vec!["update"]);
    }

    #[test]
    fn builds_operations_without_shell() {
        let ops = operations(&repo(BackendKind::Pacman), OpKind::Install, "bash", None).unwrap();
        assert_eq!(ops[0].program, "pacman");
        assert!(ops[0].privileged);
        assert!(ops[0].args.iter().any(|arg| arg == "bash"));

        let flatpak = operations(
            &repo(BackendKind::Flatpak),
            OpKind::Install,
            "org.mozilla.firefox",
            Some("flathub"),
        )
        .unwrap();
        assert!(!flatpak[0].privileged);
        assert!(flatpak[0].args.iter().any(|arg| arg == "flathub"));
    }

    #[test]
    fn custom_command_substitutes_package_and_remote() {
        let mut custom = repo(BackendKind::Custom);
        custom.overrides.install = Some("pkexec own install --remote {remote} {package}".into());
        let ops = operations(&custom, OpKind::Install, "foo", Some("stable")).unwrap();
        assert_eq!(ops[0].program, "pkexec");
        assert_eq!(
            ops[0].args,
            vec!["own", "install", "--remote", "stable", "foo"]
        );

        let ops = operations(&custom, OpKind::Install, "foo", None).unwrap();
        assert_eq!(ops[0].args, vec!["own", "install", "foo"]);
    }

    fn privileged_op() -> Operation {
        Operation {
            program: "pacman".into(),
            args: vec!["-S".into(), "bash".into()],
            privileged: true,
        }
    }

    #[test]
    fn privilege_none_runs_command_directly() {
        let (program, args) =
            apply_privilege(WorkerSettings::new(Privilege::None), &privileged_op()).unwrap();
        assert_eq!(program, "pacman");
        assert_eq!(args, vec!["-S", "bash"]);
    }

    #[test]
    fn auto_uses_pkexec_when_available() {
        if which("pkexec").is_none() {
            return;
        }
        let (program, args) =
            apply_privilege(WorkerSettings::new(Privilege::Auto), &privileged_op()).unwrap();
        assert!(program.ends_with("pkexec"), "{program}");
        // pkexec dostaje najpierw **ścieżkę** programu, potem jego argumenty.
        assert!(args[0].ends_with("pacman"), "{args:?}");
        assert_eq!(&args[1..], ["-S", "bash"]);
    }

    #[test]
    fn sudo_runs_non_interactively() {
        if which("sudo").is_none() {
            return;
        }
        let (program, args) =
            apply_privilege(WorkerSettings::new(Privilege::Sudo), &privileged_op()).unwrap();
        assert!(program.ends_with("sudo"), "{program}");
        assert_eq!(args[0], "-n");
        assert!(args[1].ends_with("pacman"));
    }

    #[test]
    fn unprivileged_operations_are_never_wrapped() {
        let op = Operation {
            program: "flatpak".into(),
            args: vec!["install".into()],
            privileged: false,
        };
        for privilege in [
            Privilege::Auto,
            Privilege::Pkexec,
            Privilege::Sudo,
            Privilege::None,
        ] {
            let (program, args) = apply_privilege(WorkerSettings::new(privilege), &op).unwrap();
            assert_eq!(program, "flatpak");
            assert_eq!(args, vec!["install"]);
        }
    }
}

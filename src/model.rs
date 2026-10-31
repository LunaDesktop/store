//! Model danych sklepu: źródła, pakiety i porównywanie wersji.
//!
//! Ten moduł nie zna procesów ani UI — dzięki temu scalanie katalogów,
//! filtrowanie i sortowanie da się testować bez okna i bez menedżerów pakietów.

use std::cmp::Ordering;
use std::fmt;

/// Obsługiwany menedżer pakietów.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BackendKind {
    Pacman,
    Pag,
    Apt,
    Dnf,
    Emerge,
    Flatpak,
    /// Własne polecenia z pliku konfiguracji.
    Custom,
}

impl BackendKind {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "pacman" | "arch" => Some(Self::Pacman),
            "pag" | "pagan" | "paganos" => Some(Self::Pag),
            "apt" | "debian" | "ubuntu" => Some(Self::Apt),
            "dnf" | "fedora" | "rhel" => Some(Self::Dnf),
            "emerge" | "gentoo" | "portage" => Some(Self::Emerge),
            "flatpak" | "flathub" => Some(Self::Flatpak),
            "custom" | "wlasny" | "własny" => Some(Self::Custom),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pacman => "pacman",
            Self::Pag => "pag",
            Self::Apt => "apt",
            Self::Dnf => "dnf",
            Self::Emerge => "emerge",
            Self::Flatpak => "flatpak",
            Self::Custom => "custom",
        }
    }

    /// Program używany do wykrywania źródła (`enabled = auto`).
    pub fn executable(self) -> &'static str {
        match self {
            Self::Pacman => "pacman",
            Self::Pag => "pag",
            Self::Apt => "apt-get",
            Self::Dnf => "dnf",
            Self::Emerge => "emerge",
            Self::Flatpak => "flatpak",
            Self::Custom => "",
        }
    }

    pub fn is_flatpak(self) -> bool {
        matches!(self, Self::Flatpak)
    }
}

impl fmt::Display for BackendKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Pakiet w postaci zwróconej przez backend (bez stanu instalacji).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawPackage {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    /// Repozytorium Flatpaka (np. `flathub`), jeśli backend je zna.
    pub remote: Option<String>,
}

impl RawPackage {
    pub fn new(id: impl Into<String>, version: impl Into<String>) -> Self {
        let id = id.into();
        Self {
            name: id.clone(),
            id,
            version: version.into(),
            description: String::new(),
            remote: None,
        }
    }

    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }

    pub fn remote(mut self, remote: impl Into<String>) -> Self {
        self.remote = Some(remote.into());
        self
    }
}

/// Pakiet gotowy do pokazania w sklepie.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package {
    pub source: String,
    pub backend: BackendKind,
    pub remote: Option<String>,
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub installed_version: Option<String>,
    pub update_version: Option<String>,
    /// Znormalizowany tekst do wyszukiwania (małe litery).
    pub search: String,
    /// Znormalizowana nazwa do sortowania.
    pub sort_name: String,
}

impl Package {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        source: impl Into<String>,
        backend: BackendKind,
        remote: Option<String>,
        id: impl Into<String>,
        name: impl Into<String>,
        version: impl Into<String>,
        description: impl Into<String>,
        installed_version: Option<String>,
        update_version: Option<String>,
    ) -> Self {
        let source = source.into();
        let id = id.into();
        let name = name.into();
        let version = version.into();
        let description = description.into();
        let mut package = Self {
            source,
            backend,
            remote,
            id,
            name,
            description,
            version,
            installed_version,
            update_version,
            search: String::new(),
            sort_name: String::new(),
        };
        package.refresh_search();
        package
    }

    /// Stabilny klucz w obrębie całego sklepu.
    pub fn key(&self) -> String {
        package_key(&self.source, &self.id)
    }

    pub fn is_installed(&self) -> bool {
        self.installed_version.is_some()
    }

    pub fn has_update(&self) -> bool {
        self.update_version.is_some()
    }

    /// Przelicza pola pomocnicze po zmianie nazwy/opisu.
    pub fn refresh_search(&mut self) {
        self.sort_name = self.name.to_lowercase();
        self.search = format!("{} {} {}", self.id, self.name, self.description).to_lowercase();
    }

    /// Czy pakiet pasuje do jednego z tokenów `featured` z konfiguracji.
    ///
    /// Zwraca `(pozycja na liście, ranga dopasowania)`. Ranga 0 to dokładna
    /// nazwa/identyfikator, 1 — ostatni segment identyfikatora (np.
    /// `org.mozilla.firefox` → `firefox`), 2 — początek nazwy, 3 — zawieranie.
    /// Opis nie bierze udziału w dopasowaniu, bo prowadzi do przypadkowych
    /// wyników (np. „Discord client with Spotify integration” dla „spotify”).
    pub fn featured_score(&self, featured: &[String]) -> Option<(usize, u8)> {
        let id = self.id.to_lowercase();
        let name = self.name.to_lowercase();
        if id.contains("baseapp")
            || name.contains("application base")
            || name.contains("runtime")
            || name.contains(" sdk")
        {
            return None;
        }
        featured.iter().enumerate().find_map(|(position, token)| {
            let token = token.trim();
            if token.len() < 3 {
                return None;
            }
            let rank = if id == token || name == token {
                0
            } else if id.rsplit('.').next() == Some(token) {
                1
            } else if name.starts_with(token) {
                2
            } else if id.contains(token) || name.contains(token) {
                3
            } else {
                return None;
            };
            Some((position, rank))
        })
    }
}

pub fn package_key(source: &str, id: &str) -> String {
    format!("{source}\u{1f}{id}")
}

/// Porównuje wersje w sposób „naturalny”: ciągi cyfr liczbowo, reszta leksykograficznie.
///
/// Nie jest to pełny parser wersji — wystarcza do wykrywania aktualizacji w katalogach,
/// które nie udostępniają gotowej listy nowych wersji.
pub fn compare_versions(a: &str, b: &str) -> Ordering {
    let mut ia = a.chars().peekable();
    let mut ib = b.chars().peekable();
    loop {
        match (ia.peek().copied(), ib.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(ca), Some(cb)) => {
                let na = ca.is_ascii_digit();
                let nb = cb.is_ascii_digit();
                let mut ta = String::new();
                let mut tb = String::new();
                if na && nb {
                    while let Some(c) = ia.peek().copied().filter(char::is_ascii_digit) {
                        ta.push(c);
                        ia.next();
                    }
                    while let Some(c) = ib.peek().copied().filter(char::is_ascii_digit) {
                        tb.push(c);
                        ib.next();
                    }
                    let ta = ta.trim_start_matches('0');
                    let tb = tb.trim_start_matches('0');
                    let ord = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                    if ord != Ordering::Equal {
                        return ord;
                    }
                } else {
                    while let Some(c) = ia.peek().copied().filter(|c| !c.is_ascii_digit()) {
                        ta.push(c);
                        ia.next();
                    }
                    while let Some(c) = ib.peek().copied().filter(|c| !c.is_ascii_digit()) {
                        tb.push(c);
                        ib.next();
                    }
                    let ord = ta.to_lowercase().cmp(&tb.to_lowercase());
                    if ord != Ordering::Equal {
                        return ord;
                    }
                }
            }
        }
    }
}

/// `true`, gdy `candidate` jest nowsze od `current`.
pub fn version_newer(candidate: &str, current: &str) -> bool {
    compare_versions(candidate, current) == Ordering::Greater
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_numeric_and_text_segments() {
        assert_eq!(compare_versions("1.10", "1.9"), Ordering::Greater);
        assert_eq!(compare_versions("1.0", "1.0"), Ordering::Equal);
        assert_eq!(compare_versions("2.0.1", "2.0"), Ordering::Greater);
        assert!(version_newer("2026.10", "2026.9"));
        assert!(!version_newer("1.0", "1.0.1"));
    }

    #[test]
    fn featured_matches_names_and_ids_case_insensitively() {
        let p = Package::new(
            "flathub",
            BackendKind::Flatpak,
            Some("flathub".into()),
            "org.mozilla.firefox",
            "Firefox",
            "130",
            "Web browser",
            None,
            None,
        );
        let featured = vec!["firefox".to_owned(), "gimp".to_owned()];
        assert_eq!(p.featured_score(&featured), Some((0, 0)));
        assert_eq!(p.featured_score(&["gimp".to_owned()]), None);

        let base = Package::new(
            "flathub",
            BackendKind::Flatpak,
            Some("flathub".into()),
            "org.mozilla.firefox.BaseApp",
            "Firefox Application Base",
            "1",
            "Shared base",
            None,
            None,
        );
        assert_eq!(base.featured_score(&featured), None);
    }

    #[test]
    fn package_search_is_lowercase_and_stable_key() {
        let p = Package::new(
            "arch",
            BackendKind::Pacman,
            None,
            "Firefox",
            "Firefox",
            "1.0",
            "Web Browser",
            None,
            None,
        );
        assert!(p.search.contains("firefox"));
        assert!(p.search.contains("web browser"));
        assert_eq!(p.key(), package_key("arch", "Firefox"));
    }
}

//! Stan aplikacji, scalanie danych i pętla klatki Luna Store.
//!
//! Podział jest ten sam co w innych aplikacjach Luny: dane i akcje żyją tutaj,
//! a rysowanie/obsługa wejścia w [`crate::ui`]. Dzięki temu filtrowanie,
//! scalanie stanu instalacji i akcje da się testować bez okna.

use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::time::{Duration, Instant, SystemTime};

use libluna_core::ThemeSettings;
use libluna_i18n::{set_locale, tr};
use libluna_platform::{run as run_app, AppConfig, Frame};
use libluna_widgets::{
    BottomSheet, Entry, EntryIcon, IconName, Menu, MenuEntry, NavEntry, NavList, ScrollView,
    StatusIcon, StatusPage, Toast, ViewMode, WindowControl,
};

use crate::backends::valid_package;
use crate::cache;
use crate::config::{Privilege, RepoConfig, StoreConfig};
use crate::model::{package_key, BackendKind, Package, RawPackage};
use crate::worker::{ActionKind, Job, JobResult, JobRunner};

/// Strona główna lub jedno źródło pakietów.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Page {
    Featured,
    All,
    Installed,
    Updates,
    Source(String),
}

impl Page {
    pub fn source(&self) -> Option<&str> {
        match self {
            Page::Source(id) => Some(id),
            _ => None,
        }
    }
}

/// Stan jednego źródła (repozytorium/menedżera).
#[derive(Debug, Clone)]
pub struct RepoState {
    pub cfg: RepoConfig,
    pub catalog_loaded: bool,
    pub status_loaded: bool,
    pub catalog_count: usize,
    pub last_refresh: Option<SystemTime>,
    /// Błąd pobrania katalogu.
    pub error: Option<String>,
    /// Błąd odczytu stanu instalacji/aktualizacji.
    pub status_error: Option<String>,
    pub warning: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ViewSignature {
    version: u64,
    page: Page,
    query: String,
}

/// Zmiany, które trzeba zastosować do `Frame` po obsłudze zdarzeń.
#[derive(Debug, Default)]
pub struct Pending {
    pub close: bool,
    pub minimize: bool,
    pub maximize: bool,
    pub begin_move: bool,
}

/// Stan aplikacji.
pub struct App {
    pub config: StoreConfig,
    pub repos: Vec<RepoState>,
    pub page: Page,
    pub nav: NavList,
    pub nav_pages: Vec<Option<Page>>,
    /// Czy pasek boczny jest widoczny (przełącznik w headerbarze).
    pub sidebar_visible: bool,
    /// Menu ☰ w headerbarze.
    pub menu: Menu,
    pub menu_open: bool,
    /// Siatka albo lista (przełącznik w headerbarze).
    pub view_mode: ViewMode,
    pub search: Entry,
    pub scroll: ScrollView,
    pub packages: Vec<Package>,
    pub view: Vec<usize>,
    pub data_version: u64,
    pub dirty: bool,
    view_signature: Option<ViewSignature>,
    /// Liczba trwających zadań na źródło (katalog/status/opis).
    pub loading: HashMap<String, usize>,
    /// Etykieta trwającej instalacji/usuwania/aktualizacji.
    pub working: Option<String>,
    pub selected: Option<String>,
    pub confirm_remove: bool,
    pub sheet: BottomSheet,
    pub toast: Toast,
    pub toast_until: Option<Instant>,
    pub spinner: f64,
    pub pending: Pending,
    pub hover_control: Option<WindowControl>,
    pub hover_card: Option<usize>,
    pub hover_action: bool,
    pub hover_refresh: bool,
    pub hover_sidebar: bool,
    pub hover_menu: bool,
    pub hover_back: bool,
    pub hover_forward: bool,
    pub hover_view: bool,
    pub hover_update_all: bool,
    pub clipboard_text: Option<String>,
    pub paste_requested: bool,
    pub title: String,
    started: bool,
    /// Historia nawigacji (Wstecz/Dalej jak w Luna Files).
    history: Vec<Page>,
    history_pos: usize,
    pub(crate) raw: HashMap<String, Vec<RawPackage>>,
    pub(crate) installed: HashMap<String, HashMap<String, String>>,
    pub(crate) updates: HashMap<String, HashMap<String, String>>,
    descriptions: HashMap<String, String>,
    index: HashMap<String, usize>,
    runner: JobRunner,
}

impl App {
    pub fn new() -> Self {
        Self::from_config(StoreConfig::load())
    }

    pub fn from_config(config: StoreConfig) -> Self {
        let mut repos = Vec::new();
        let mut raw = HashMap::new();
        for cfg in config.active_repos() {
            let cached = cache::load(&cfg.id, cfg.backend);
            let loaded = !cached.is_empty();
            let count = cached.len();
            if loaded {
                raw.insert(cfg.id.clone(), cached);
            }
            repos.push(RepoState {
                catalog_loaded: loaded,
                catalog_count: count,
                status_loaded: false,
                last_refresh: cache::stamp(&cfg.id),
                error: None,
                status_error: None,
                warning: None,
                cfg,
            });
        }

        let privilege = config.privilege;
        let mut app = Self {
            config,
            repos,
            page: Page::Featured,
            nav: NavList::default(),
            nav_pages: Vec::new(),
            sidebar_visible: true,
            menu: Menu::new(menu_entries()),
            menu_open: false,
            view_mode: ViewMode::Grid,
            search: Entry::new(tr("Search apps…"))
                .leading(EntryIcon::Search)
                .clearable(true),
            scroll: ScrollView::new(),
            packages: Vec::new(),
            view: Vec::new(),
            data_version: 0,
            dirty: true,
            view_signature: None,
            loading: HashMap::new(),
            working: None,
            selected: None,
            confirm_remove: false,
            sheet: BottomSheet::new(),
            toast: Toast::default(),
            toast_until: None,
            spinner: 0.0,
            pending: Pending::default(),
            hover_control: None,
            hover_card: None,
            hover_action: false,
            hover_refresh: false,
            hover_sidebar: false,
            hover_menu: false,
            hover_back: false,
            hover_forward: false,
            hover_view: false,
            hover_update_all: false,
            clipboard_text: None,
            paste_requested: false,
            title: tr("Store"),
            started: false,
            history: vec![Page::Featured],
            history_pos: 0,
            raw,
            installed: HashMap::new(),
            updates: HashMap::new(),
            descriptions: HashMap::new(),
            index: HashMap::new(),
            runner: JobRunner::new(privilege),
        };
        let sources: Vec<String> = app.repos.iter().map(|repo| repo.cfg.id.clone()).collect();
        for source in sources {
            app.rebuild_source_packages(&source);
        }
        app.rebuild_nav();
        app.sync_view();
        app
    }

    /// Uruchamia pierwsze zapytania (cache jest już na ekranie).
    pub fn start_initial_jobs(&mut self) {
        if self.started {
            return;
        }
        self.started = true;
        let repos: Vec<RepoConfig> = self.repos.iter().map(|repo| repo.cfg.clone()).collect();
        for repo in repos {
            let cached_empty = self.raw.get(&repo.id).map(Vec::is_empty).unwrap_or(true);
            if cached_empty || cache::is_stale(&repo.id, self.config.refresh_minutes) {
                self.spawn_catalog(repo.clone());
            }
            self.spawn_status(repo);
        }
    }

    pub fn refresh_all(&mut self) {
        if self.working.is_some() {
            self.show_toast(tr("Another operation is running"));
            return;
        }
        if self.repos.is_empty() {
            self.show_toast(tr("No package manager detected"));
            return;
        }
        let repos: Vec<RepoConfig> = self.repos.iter().map(|repo| repo.cfg.clone()).collect();
        for repo in repos {
            cache::invalidate(&repo.id);
            self.spawn_catalog(repo.clone());
            self.spawn_status(repo);
        }
    }

    pub fn update_all(&mut self) {
        if self.working.is_some() {
            self.show_toast(tr("Another operation is running"));
            return;
        }
        let repos: Vec<RepoConfig> = self
            .repos
            .iter()
            .filter(|repo| {
                self.updates
                    .get(&repo.cfg.id)
                    .map(|updates| !updates.is_empty())
                    .unwrap_or(false)
            })
            .map(|repo| repo.cfg.clone())
            .collect();
        if repos.is_empty() {
            self.show_toast(tr("No updates"));
            return;
        }
        self.working = Some(tr("Updating all sources…"));
        self.close_details();
        self.runner.spawn(Job::UpdateAll(repos));
    }

    pub fn start_action(&mut self, package: &Package, kind: ActionKind) {
        if self.working.is_some() {
            self.show_toast(tr("Another operation is running"));
            return;
        }
        if !valid_package(&package.id) {
            self.show_toast(format!("{}: {}", tr("Failed"), package.id));
            return;
        }
        let Some(repo) = self
            .repos
            .iter()
            .find(|repo| repo.cfg.id == package.source)
            .map(|repo| repo.cfg.clone())
        else {
            self.show_toast(tr("Source is not available"));
            return;
        };
        let label = match kind {
            ActionKind::Install => tr("Installing…"),
            ActionKind::Remove => tr("Removing…"),
            ActionKind::Update => tr("Updating…"),
            ActionKind::UpdateAll => tr("Updating all sources…"),
        };
        // Zmiany systemowe wymagają autoryzacji (pkexec → agent polkit Luny).
        if self.config.privilege != Privilege::None && needs_admin(&repo) {
            self.show_toast(tr("Authorization requested — check the system dialog"));
        }
        self.working = Some(format!("{label} {}", package.name));
        self.close_details();
        self.search.set_focused(false);
        self.runner.spawn(Job::Action {
            repo,
            kind,
            id: package.id.clone(),
            name: package.name.clone(),
            remote: package
                .remote
                .clone()
                .or_else(|| package_remote(&package.source, &self.repos)),
        });
    }

    /// Klik w przycisk karty.
    pub fn card_action(&mut self, view_index: usize) {
        let Some(&index) = self.view.get(view_index) else {
            return;
        };
        let package = self.packages[index].clone();
        if package.has_update() {
            self.start_action(&package, ActionKind::Update);
        } else if !package.is_installed() {
            self.start_action(&package, ActionKind::Install);
        } else {
            let key = package.key();
            self.open_details(&key);
        }
    }

    pub fn open_details(&mut self, key: &str) {
        let Some(index) = self.index.get(key).copied() else {
            return;
        };
        let package = &self.packages[index];
        let name = package.name.clone();
        let id = package.id.clone();
        let source = package.source.clone();
        let backend = package.backend;
        let needs_description = package.description.is_empty();
        self.selected = Some(key.to_owned());
        self.confirm_remove = false;
        self.menu_open = false;
        // Panel jest modalny — pole wyszukiwania nie może zostać aktywne pod nim.
        self.search.set_focused(false);
        self.sheet.set_title(Some(name));
        // Opis + metadane + przyciski potrzebują więcej miejsca niż domyślny panel.
        self.sheet.set_height(340.0);
        self.sheet.set_open_animated(true);
        if needs_description
            && matches!(
                backend,
                BackendKind::Pacman | BackendKind::Dnf | BackendKind::Emerge
            )
        {
            if let Some(repo) = self
                .repos
                .iter()
                .find(|repo| repo.cfg.id == source)
                .map(|repo| repo.cfg.clone())
            {
                self.spawn_describe(repo, id);
            }
        }
    }

    pub fn close_details(&mut self) {
        self.sheet.set_open_animated(false);
        self.confirm_remove = false;
    }

    pub fn selected_package(&self) -> Option<&Package> {
        self.selected
            .as_ref()
            .and_then(|key| self.index.get(key))
            .map(|index| &self.packages[*index])
    }

    /// Czy akcja dla wybranego pakietu wymaga hasła administratora.
    ///
    /// Pokazujemy to w panelu szczegółów, żeby użytkownik wiedział, że po
    /// „Zainstaluj” pojawi się okno autoryzacji polkit.
    pub fn action_requires_admin(&self) -> bool {
        let Some(package) = self.selected_package() else {
            return false;
        };
        self.config.privilege != Privilege::None
            && self
                .repos
                .iter()
                .find(|repo| repo.cfg.id == package.source)
                .map_or(true, |repo| needs_admin(&repo.cfg))
    }

    pub fn detail_primary(&mut self) {
        let Some(package) = self.selected_package().cloned() else {
            return;
        };
        if self.confirm_remove {
            self.start_action(&package, ActionKind::Remove);
        } else if package.has_update() {
            self.start_action(&package, ActionKind::Update);
        } else if package.is_installed() {
            self.confirm_remove = true;
        } else {
            self.start_action(&package, ActionKind::Install);
        }
    }

    pub fn detail_secondary(&mut self) {
        if self.confirm_remove {
            self.confirm_remove = false;
        } else if self
            .selected_package()
            .is_some_and(|package| package.is_installed() && package.has_update())
        {
            self.confirm_remove = true;
        }
    }

    pub fn go_to(&mut self, page: Page) {
        self.navigate(page, true);
    }

    fn navigate(&mut self, page: Page, record: bool) {
        if self.page == page {
            return;
        }
        if record {
            self.history.truncate(self.history_pos + 1);
            self.history.push(page.clone());
            if self.history.len() > 64 {
                let drop = self.history.len() - 64;
                self.history.drain(0..drop);
            }
            self.history_pos = self.history.len() - 1;
        }
        self.page = page;
        // Nowa strona czyści wyszukiwanie — pole nie zostaje z nieaktualnym filtrem.
        self.search.clear();
        self.search.set_focused(false);
        self.scroll = ScrollView::new();
        self.dirty = true;
        self.select_nav();
        self.sync_view();
    }

    pub fn can_go_back(&self) -> bool {
        self.history_pos > 0
    }

    pub fn can_go_forward(&self) -> bool {
        self.history_pos + 1 < self.history.len()
    }

    pub fn go_back(&mut self) {
        if !self.can_go_back() {
            return;
        }
        self.history_pos -= 1;
        let page = self.history[self.history_pos].clone();
        self.navigate(page, false);
    }

    pub fn go_forward(&mut self) {
        if !self.can_go_forward() {
            return;
        }
        self.history_pos += 1;
        let page = self.history[self.history_pos].clone();
        self.navigate(page, false);
    }

    /// Przełącza siatkę i listę (ikona w headerbarze).
    pub fn toggle_view_mode(&mut self) {
        self.view_mode = match self.view_mode {
            ViewMode::Grid => ViewMode::List,
            ViewMode::List => ViewMode::Grid,
        };
        self.scroll = ScrollView::new();
    }

    pub fn on_query_changed(&mut self) {
        self.scroll = ScrollView::new();
        self.dirty = true;
        // Bez tego nowy filtr byłby widoczny dopiero po następnym wyniku
        // z wątku — a przy samym wpisywaniu żaden wynik nie nadchodzi.
        self.sync_view();
    }

    pub fn sync_view(&mut self) {
        let query = self.search.text().trim().to_lowercase();
        let signature = ViewSignature {
            version: self.data_version,
            page: self.page.clone(),
            query: query.clone(),
        };
        if !self.dirty && self.view_signature.as_ref() == Some(&signature) {
            return;
        }

        let mut view: Vec<usize> = match &self.page {
            Page::Featured => self.featured_view(),
            Page::All => (0..self.packages.len()).collect(),
            Page::Installed => self
                .packages
                .iter()
                .enumerate()
                .filter(|(_, package)| package.is_installed())
                .map(|(index, _)| index)
                .collect(),
            Page::Updates => self
                .packages
                .iter()
                .enumerate()
                .filter(|(_, package)| package.has_update())
                .map(|(index, _)| index)
                .collect(),
            Page::Source(source) => self
                .packages
                .iter()
                .enumerate()
                .filter(|(_, package)| &package.source == source)
                .map(|(index, _)| index)
                .collect(),
        };

        if !query.is_empty() {
            // Wyszukiwanie jest globalne: szukamy we **wszystkich** źródłach, nie
            // tylko na bieżącej stronie (inaczej np. „Odkrywaj” nie znajduje
            // niczego poza polecanymi aplikacjami).
            view = (0..self.packages.len())
                .filter(|index| self.packages[*index].search.contains(&query))
                .collect();
            view.sort_by(|a, b| {
                let left = &self.packages[*a];
                let right = &self.packages[*b];
                search_rank(left, &query)
                    .cmp(&search_rank(right, &query))
                    .then_with(|| left.sort_name.cmp(&right.sort_name))
                    .then_with(|| left.id.cmp(&right.id))
            });
        } else if self.page != Page::Featured {
            view.sort_by(|a, b| {
                self.packages[*a]
                    .sort_name
                    .cmp(&self.packages[*b].sort_name)
                    .then_with(|| self.packages[*a].id.cmp(&self.packages[*b].id))
            });
        }

        self.view = view;
        self.view_signature = Some(signature);
        self.dirty = false;
    }

    fn featured_view(&self) -> Vec<usize> {
        let featured = self.config.featured.clone();
        let mut scored: Vec<(usize, u8, usize)> = Vec::new();
        if !featured.is_empty() {
            for (index, package) in self.packages.iter().enumerate() {
                if let Some((position, rank)) = package.featured_score(&featured) {
                    scored.push((position, rank, index));
                }
            }
            scored.sort_by(|(position_a, rank_a, a), (position_b, rank_b, b)| {
                position_a
                    .cmp(position_b)
                    .then(rank_a.cmp(rank_b))
                    .then_with(|| {
                        self.packages[*a]
                            .sort_name
                            .cmp(&self.packages[*b].sort_name)
                    })
            });
        }
        let mut seen = HashSet::new();
        let mut view: Vec<usize> = scored
            .into_iter()
            .map(|(_, _, index)| index)
            .filter(|index| seen.insert(self.packages[*index].key()))
            .collect();
        view.truncate(120);

        if view.is_empty() {
            // Brak dopasowań do listy `featured` — pokaż pakiet z opisem,
            // żeby pierwszy ekran nie był pusty na małych repozytoriach.
            let mut fallback: Vec<usize> = self
                .packages
                .iter()
                .enumerate()
                .filter(|(_, package)| !package.description.is_empty())
                .map(|(index, _)| index)
                .collect();
            fallback.sort_by(|a, b| {
                self.packages[*a]
                    .sort_name
                    .cmp(&self.packages[*b].sort_name)
                    .then_with(|| self.packages[*a].id.cmp(&self.packages[*b].id))
            });
            fallback.truncate(60);
            view = fallback;
        }
        view
    }

    pub fn poll_jobs(&mut self) {
        let results = self.runner.poll();
        if results.is_empty() {
            return;
        }
        for result in results {
            match result {
                JobResult::Catalog {
                    source,
                    packages,
                    error,
                } => {
                    self.end_load(&source);
                    if let Some(error) = &error {
                        if let Some(repo) = self.repo_mut(&source) {
                            repo.error = Some(error.clone());
                        }
                    }
                    if !packages.is_empty() {
                        if let Some(backend) = self
                            .repos
                            .iter()
                            .find(|repo| repo.cfg.id == source)
                            .map(|repo| repo.cfg.backend)
                        {
                            cache::save(&source, backend, &packages);
                        }
                        let count = packages.len();
                        self.raw.insert(source.clone(), packages);
                        if let Some(repo) = self.repo_mut(&source) {
                            repo.catalog_loaded = true;
                            repo.catalog_count = count;
                            repo.last_refresh = Some(SystemTime::now());
                            if error.is_none() {
                                repo.error = None;
                            }
                        }
                        self.rebuild_source_packages(&source);
                        self.data_version = self.data_version.wrapping_add(1);
                        self.dirty = true;
                        self.rebuild_nav();
                    }
                }
                JobResult::Status {
                    source,
                    installed,
                    updates,
                    error,
                    warning,
                } => {
                    self.end_load(&source);
                    if let Some(error) = error {
                        if let Some(repo) = self.repo_mut(&source) {
                            repo.status_error = Some(error);
                        }
                    } else if let (Some(installed), Some(updates)) = (installed, updates) {
                        self.installed.insert(
                            source.clone(),
                            installed.into_iter().collect::<HashMap<_, _>>(),
                        );
                        self.updates.insert(
                            source.clone(),
                            updates.into_iter().collect::<HashMap<_, _>>(),
                        );
                        if let Some(repo) = self.repo_mut(&source) {
                            repo.status_loaded = true;
                            repo.status_error = None;
                            repo.warning = warning;
                        }
                        self.rebuild_source_packages(&source);
                        self.data_version = self.data_version.wrapping_add(1);
                        self.dirty = true;
                        self.rebuild_nav();
                    }
                }
                JobResult::Described {
                    source,
                    id,
                    description,
                } => {
                    self.end_load(&source);
                    if !description.is_empty() {
                        let key = package_key(&source, &id);
                        self.descriptions.insert(key.clone(), description.clone());
                        if let Some(index) = self.index.get(&key).copied() {
                            self.packages[index].description = description;
                            self.packages[index].refresh_search();
                            self.data_version = self.data_version.wrapping_add(1);
                            self.dirty = true;
                        }
                    }
                }
                JobResult::ActionDone(outcome) => {
                    self.working = None;
                    if outcome.success {
                        let message = match outcome.kind {
                            ActionKind::Install => {
                                format!("{}: {}", tr("Installed"), outcome.name)
                            }
                            ActionKind::Remove => {
                                format!("{}: {}", tr("Removed"), outcome.name)
                            }
                            ActionKind::Update => {
                                format!("{}: {}", tr("Updated"), outcome.name)
                            }
                            ActionKind::UpdateAll => tr("All updates installed"),
                        };
                        self.show_toast(message);
                        if outcome.kind == ActionKind::UpdateAll {
                            self.refresh_statuses();
                        } else if let Some(source) = outcome.source {
                            self.refresh_status(&source);
                        }
                        self.close_details();
                    } else {
                        let detail = outcome.detail.unwrap_or_default();
                        let first = detail.lines().next().unwrap_or("").trim().to_owned();
                        self.show_toast(format!("{}: {}", tr("Failed"), first));
                    }
                }
            }
        }
        self.sync_view();
    }

    pub fn tick(&mut self, dt: f64) {
        self.spinner = (self.spinner + dt * 0.75).rem_euclid(1.0);
        // Bez tego panel szczegółów i powiadomienia zostają na progress = 0,
        // czyli są „otwarte”, ale niewidoczne (i blokują wejście jak modal).
        self.sheet.advance(dt, 0.22);
        self.toast.advance(dt, 0.28);
        if let Some(until) = self.toast_until {
            if Instant::now() >= until {
                self.toast.set_open_animated(false);
                self.toast_until = None;
            }
        }
    }

    pub fn needs_frames(&self) -> bool {
        !self.loading.is_empty()
            || self.working.is_some()
            || self.sheet.is_animating()
            || self.toast.is_animating()
            || self.toast_until.is_some()
    }

    pub fn apply_pending(&mut self, frame: &mut Frame) {
        if self.pending.close {
            self.pending.close = false;
            frame.close();
        }
        if self.pending.minimize {
            self.pending.minimize = false;
            frame.minimize();
        }
        if self.pending.maximize {
            self.pending.maximize = false;
            frame.toggle_maximize();
        }
        if self.pending.begin_move {
            self.pending.begin_move = false;
            frame.begin_move();
        }
    }

    pub fn status_page(&self) -> Option<StatusPage> {
        if self.repos.is_empty() {
            return Some(
                StatusPage::new(tr("No package manager detected"))
                    .icon(StatusIcon::Folder)
                    .description(tr(
                        "Install pacman, pag, apt, dnf, emerge or Flatpak, then refresh.",
                    ))
                    .action(tr("Refresh")),
            );
        }
        if !self.view.is_empty() {
            return None;
        }
        if !self.loading.is_empty() || self.working.is_some() {
            return Some(
                StatusPage::new(tr("Loading…"))
                    .icon(StatusIcon::Search)
                    .description(tr("Loading packages from your repositories")),
            );
        }
        if !self.search.text().trim().is_empty() {
            return Some(
                StatusPage::new(tr("No apps match the filter"))
                    .icon(StatusIcon::Search)
                    .description(tr("Try another phrase or clear the search."))
                    .action(tr("Clear search")),
            );
        }
        match self.page {
            Page::Updates => Some(
                StatusPage::new(tr("Everything is up to date"))
                    .icon(StatusIcon::File)
                    .description(tr("No updates are available right now.")),
            ),
            Page::Installed => Some(
                StatusPage::new(tr("No installed packages"))
                    .icon(StatusIcon::Folder)
                    .description(tr("Install something from the All apps page."))
                    .action(tr("Show all apps")),
            ),
            Page::Featured => Some(
                StatusPage::new(tr("No featured apps"))
                    .icon(StatusIcon::Folder)
                    .description(tr("Your sources did not return any applications."))
                    .action(tr("Show all apps")),
            ),
            _ => Some(
                StatusPage::new(tr("No packages"))
                    .icon(StatusIcon::Folder)
                    .description(tr("Refresh the sources or check your connection."))
                    .action(tr("Refresh")),
            ),
        }
    }

    /// Obsługa pozycji menu ☰ (indeksy zgodne z [`menu_entries`]).
    pub fn menu_action(&mut self, index: usize) {
        match index {
            0 => self.refresh_all(),
            2 => self.go_to(Page::Featured),
            3 => self.go_to(Page::All),
            4 => self.go_to(Page::Installed),
            5 => self.go_to(Page::Updates),
            7 => self.open_configuration(),
            8 => self.show_toast(format!(
                "Luna Store {} · GPL-3.0-or-later",
                env!("CARGO_PKG_VERSION")
            )),
            _ => {}
        }
    }

    fn open_configuration(&mut self) {
        let Some(path) = self.config.path.clone() else {
            self.show_toast(tr("No configuration file"));
            return;
        };
        match std::process::Command::new("xdg-open").arg(&path).spawn() {
            Ok(_) => self.show_toast(format!("{}: {}", tr("Opened"), path.display())),
            Err(_) => self.show_toast(tr("Could not open configuration")),
        }
    }

    pub fn status_action(&mut self) {
        if self.repos.is_empty() {
            self.refresh_all();
            return;
        }
        if !self.search.text().trim().is_empty() {
            self.search.clear();
            self.on_query_changed();
            return;
        }
        if matches!(self.page, Page::Installed | Page::Featured) {
            self.go_to(Page::All);
        } else {
            self.refresh_all();
        }
    }

    /// Wstrzykuje dane źródła bez uruchamiania wątków (podgląd/headless, testy).
    #[doc(hidden)]
    pub fn preview_data(
        &mut self,
        source: &str,
        packages: Vec<RawPackage>,
        installed: Vec<(String, String)>,
        updates: Vec<(String, String)>,
    ) {
        self.raw.insert(source.to_owned(), packages);
        self.installed
            .insert(source.to_owned(), installed.into_iter().collect());
        self.updates
            .insert(source.to_owned(), updates.into_iter().collect());
        self.rebuild_source_packages(source);
        self.data_version = self.data_version.wrapping_add(1);
        self.dirty = true;
        self.rebuild_nav();
        self.sync_view();
    }

    pub fn is_loading(&self) -> bool {
        !self.loading.is_empty()
    }

    pub fn update_count(&self) -> usize {
        self.packages
            .iter()
            .filter(|package| package.has_update())
            .count()
    }

    pub fn repo_name<'a>(&'a self, source: &str) -> &'a str {
        self.repos
            .iter()
            .find(|repo| repo.cfg.id == source)
            .map(|repo| repo.cfg.name.as_str())
            .unwrap_or("")
    }

    pub fn error_text(&self) -> Option<&str> {
        self.repos.iter().find_map(|repo| {
            repo.error
                .as_deref()
                .or(repo.status_error.as_deref())
                .or(repo.warning.as_deref())
        })
    }

    pub fn show_toast(&mut self, message: String) {
        let message = truncate(&message, 160);
        self.toast = Toast::new(message);
        self.toast.show_animated();
        self.toast_until = Some(Instant::now() + Duration::from_secs(4));
    }

    pub(crate) fn rebuild_nav(&mut self) {
        let updates = self.update_count();
        let mut entries = Vec::new();
        let mut icons = Vec::new();
        let mut pages = Vec::new();

        entries.push(NavEntry::Section(tr("Store")));
        icons.push(None);
        pages.push(None);

        let updates_label = if updates > 0 {
            format!("{} ({updates})", tr("Updates"))
        } else {
            tr("Updates")
        };
        for (label, icon, page) in [
            (tr("Featured"), IconName::Star, Page::Featured),
            (tr("All apps"), IconName::Store, Page::All),
            (tr("Installed"), IconName::Check, Page::Installed),
            (updates_label, IconName::Clock, Page::Updates),
        ] {
            entries.push(NavEntry::Item(label));
            icons.push(Some(icon));
            pages.push(Some(page));
        }

        if !self.repos.is_empty() {
            entries.push(NavEntry::Section(tr("Sources")));
            icons.push(None);
            pages.push(None);
            for repo in &self.repos {
                entries.push(NavEntry::Item(repo.cfg.name.clone()));
                icons.push(Some(source_icon(repo.cfg.backend)));
                pages.push(Some(Page::Source(repo.cfg.id.clone())));
            }
        }

        self.nav = NavList::new(entries);
        self.nav.set_icons(icons);
        self.nav_pages = pages;
        self.select_nav();
    }

    fn select_nav(&mut self) {
        if let Some(index) = self
            .nav_pages
            .iter()
            .position(|page| page.as_ref() == Some(&self.page))
        {
            self.nav.set_selected(Some(index));
            return;
        }
        if self.page != Page::All {
            self.page = Page::All;
            self.dirty = true;
            if let Some(index) = self
                .nav_pages
                .iter()
                .position(|page| page.as_ref() == Some(&Page::All))
            {
                self.nav.set_selected(Some(index));
            }
        }
    }

    pub(crate) fn rebuild_source_packages(&mut self, source: &str) {
        let Some((backend, remote_default)) = self
            .repos
            .iter()
            .find(|repo| repo.cfg.id == source)
            .map(|repo| (repo.cfg.backend, repo.cfg.remote.clone()))
        else {
            return;
        };
        // Celowo nie klonujemy katalogu: przy 100 000 pakietów kopia przy
        // każdym odświeżeniu stanu instalacji byłaby zauważalna.
        self.packages.retain(|package| package.source != source);
        let mut present = HashSet::new();
        {
            let raw = self.raw.get(source);
            let installed = self.installed.get(source);
            let updates = self.updates.get(source);

            if let Some(raw) = raw {
                for item in raw {
                    let key = package_key(source, &item.id);
                    let description = self
                        .descriptions
                        .get(&key)
                        .filter(|description| !description.is_empty())
                        .cloned()
                        .unwrap_or_else(|| item.description.clone());
                    present.insert(item.id.clone());
                    self.packages.push(Package::new(
                        source.to_owned(),
                        backend,
                        item.remote.clone().or_else(|| remote_default.clone()),
                        item.id.clone(),
                        item.name.clone(),
                        item.version.clone(),
                        description,
                        installed.and_then(|map| map.get(&item.id).cloned()),
                        updates.and_then(|map| map.get(&item.id).cloned()),
                    ));
                }
            }

            if let Some(installed) = installed {
                for (id, version) in installed {
                    if present.contains(id) {
                        continue;
                    }
                    let key = package_key(source, id);
                    let description = self.descriptions.get(&key).cloned().unwrap_or_default();
                    self.packages.push(Package::new(
                        source.to_owned(),
                        backend,
                        remote_default.clone(),
                        id.clone(),
                        id.clone(),
                        version.clone(),
                        description,
                        Some(version.clone()),
                        updates.and_then(|map| map.get(id).cloned()),
                    ));
                }
            }
        }

        self.rebuild_index();
        self.dirty = true;
    }

    fn rebuild_index(&mut self) {
        self.index.clear();
        for (index, package) in self.packages.iter().enumerate() {
            self.index.insert(package.key(), index);
        }
    }

    fn repo_mut(&mut self, id: &str) -> Option<&mut RepoState> {
        self.repos.iter_mut().find(|repo| repo.cfg.id == id)
    }

    fn spawn_catalog(&mut self, repo: RepoConfig) {
        self.begin_load(&repo.id);
        self.runner.spawn(Job::Catalog(repo));
    }

    fn spawn_status(&mut self, repo: RepoConfig) {
        self.begin_load(&repo.id);
        self.runner.spawn(Job::Status(repo));
    }

    fn spawn_describe(&mut self, repo: RepoConfig, id: String) {
        self.begin_load(&repo.id);
        self.runner.spawn(Job::Describe(repo, id));
    }

    fn refresh_status(&mut self, source: &str) {
        if let Some(repo) = self
            .repos
            .iter()
            .find(|repo| repo.cfg.id == source)
            .map(|repo| repo.cfg.clone())
        {
            self.spawn_status(repo);
        }
    }

    fn refresh_statuses(&mut self) {
        let repos: Vec<RepoConfig> = self.repos.iter().map(|repo| repo.cfg.clone()).collect();
        for repo in repos {
            self.spawn_status(repo);
        }
    }

    fn begin_load(&mut self, source: &str) {
        *self.loading.entry(source.to_owned()).or_insert(0) += 1;
    }

    fn end_load(&mut self, source: &str) {
        let remove = match self.loading.get_mut(source) {
            Some(count) => {
                *count = count.saturating_sub(1);
                *count == 0
            }
            None => false,
        };
        if remove {
            self.loading.remove(source);
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

/// Pozycje menu ☰. Kolejność musi być zgodna z [`App::menu_action`].
fn menu_entries() -> Vec<MenuEntry> {
    vec![
        MenuEntry::action(tr("Refresh")).shortcut("Ctrl+R"),
        MenuEntry::separator(),
        MenuEntry::action(tr("Featured")),
        MenuEntry::action(tr("All apps")),
        MenuEntry::action(tr("Installed")),
        MenuEntry::action(tr("Updates")),
        MenuEntry::separator(),
        MenuEntry::action(tr("Open repository configuration")),
        MenuEntry::action(tr("About")),
    ]
}

fn package_remote(source: &str, repos: &[RepoState]) -> Option<String> {
    repos
        .iter()
        .find(|repo| repo.cfg.id == source)
        .and_then(|repo| repo.cfg.remote.clone())
}

/// Czy operacja w tym źródle będzie wymagała uprawnień administratora.
///
/// Flatpak w trybie użytkownika instaluje bez hasła; systemowy (domyślny dla
/// repozytoriów) prosi o autoryzację przez polkit. Menadżery systemowe zawsze.
fn needs_admin(repo: &RepoConfig) -> bool {
    if repo.backend != BackendKind::Flatpak {
        return true;
    }
    !repo
        .installation
        .as_deref()
        .is_some_and(|scope| scope.eq_ignore_ascii_case("user"))
}

/// Trafność wyniku: dokładne trafienie, prefiks, nazwa, reszta (opis/ID).
fn search_rank(package: &Package, query: &str) -> u8 {
    let id = package.id.to_lowercase();
    let name = package.name.to_lowercase();
    if id == query || name == query {
        0
    } else if id.starts_with(query) || name.starts_with(query) {
        1
    } else if name.contains(query) {
        2
    } else {
        3
    }
}

pub(crate) fn source_icon(backend: BackendKind) -> IconName {
    match backend {
        BackendKind::Flatpak => IconName::Globe,
        BackendKind::Custom => IconName::App,
        _ => IconName::Archive,
    }
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Uruchamia Luna Store.
pub fn run() -> Result<(), Box<dyn Error>> {
    set_locale(libluna_i18n::preferred_locale());
    libluna_i18n::load_catalogs_from_dirs();

    let settings = ThemeSettings::load();
    let system_dark = std::env::var("LUNA_SYSTEM_DARK")
        .map(|value| !value.is_empty() && value != "0")
        .unwrap_or(false);

    let mut app = App::new();
    app.start_initial_jobs();
    let mut ui_language = libluna_i18n::locale().language().to_owned();
    let title = tr("Store");

    let config = AppConfig::new(title.clone())
        .app_id("eu.luna.Store")
        .initial_size(1120, 760)
        .settings(settings)
        .system_dark(system_dark)
        // Esc zamyka panel szczegółów, a nie cały sklep.
        .escape_to_exit(false);

    run_app(config, move |frame: &mut Frame| {
        let language = libluna_i18n::locale().language().to_owned();
        if language != ui_language {
            ui_language = language;
            app.title = tr("Store");
            app.search.set_placeholder(tr("Search apps…"));
            app.menu = Menu::new(menu_entries());
            app.rebuild_nav();
            frame.set_title(app.title.clone());
        }

        app.poll_jobs();
        crate::ui::handle_events(&mut app, frame.events, frame.size, &frame.theme, frame.text);

        if let Some(text) = app.clipboard_text.take() {
            frame.copy_to_clipboard(text);
        }
        if app.paste_requested {
            app.paste_requested = false;
            frame.paste_from_clipboard();
        }

        app.apply_pending(frame);
        app.tick(frame.delta_time());
        crate::ui::paint(&app, frame.scene, frame.size, &frame.theme, frame.text);

        if app.needs_frames() {
            // Animacje paneli/powiadomień potrzebują płynnych klatek (60 fps),
            // a postęp zadań wystarczy odświeżać rzadziej.
            let delay = if app.sheet.is_animating() || app.toast.is_animating() {
                Duration::from_millis(16)
            } else {
                Duration::from_millis(80)
            };
            frame.request_frame_after(delay);
        } else {
            frame.request_frame_after(Duration::from_secs(1));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Commands, Enabled, Privilege};
    use crate::model::BackendKind;

    fn repo(id: &str, name: &str, backend: BackendKind) -> RepoConfig {
        RepoConfig {
            id: id.into(),
            name: name.into(),
            backend,
            enabled: Enabled::Yes,
            remote: None,
            installation: None,
            probe: None,
            overrides: Commands::default(),
        }
    }

    fn config(repos: Vec<RepoConfig>) -> StoreConfig {
        StoreConfig {
            refresh_minutes: 60,
            privilege: Privilege::None,
            featured: vec!["firefox".into()],
            repos,
            path: None,
            warning: None,
        }
    }

    fn test_app() -> App {
        let source = format!("test-{}", std::process::id());
        let mut app = App::from_config(config(vec![repo(&source, "Test", BackendKind::Pacman)]));
        app.raw.insert(
            source.clone(),
            vec![
                RawPackage {
                    id: "firefox".into(),
                    name: "Firefox".into(),
                    version: "130".into(),
                    description: "Web browser".into(),
                    remote: None,
                },
                RawPackage {
                    id: "gimp".into(),
                    name: "GIMP".into(),
                    version: "2.10".into(),
                    description: "Image editor".into(),
                    remote: None,
                },
            ],
        );
        app.installed.insert(
            source.clone(),
            [("gimp".to_owned(), "2.10".to_owned())]
                .into_iter()
                .collect(),
        );
        app.updates.insert(
            source.clone(),
            [("gimp".to_owned(), "2.12".to_owned())]
                .into_iter()
                .collect(),
        );
        app.rebuild_source_packages(&source);
        app.data_version += 1;
        app.dirty = true;
        app.rebuild_nav();
        app.sync_view();
        app
    }

    #[test]
    fn merges_catalog_installed_and_updates() {
        let app = test_app();
        assert_eq!(app.packages.len(), 2);
        let firefox = app
            .packages
            .iter()
            .find(|package| package.id == "firefox")
            .unwrap();
        assert!(!firefox.is_installed());
        let gimp = app
            .packages
            .iter()
            .find(|package| package.id == "gimp")
            .unwrap();
        assert!(gimp.is_installed());
        assert_eq!(gimp.update_version.as_deref(), Some("2.12"));
        assert_eq!(app.update_count(), 1);
    }

    #[test]
    fn pages_filter_and_search() {
        let mut app = test_app();
        app.go_to(Page::Installed);
        assert_eq!(app.view.len(), 1);
        app.go_to(Page::Updates);
        assert_eq!(app.view.len(), 1);
        app.go_to(Page::All);
        app.search.set_text("fire");
        app.on_query_changed();
        assert_eq!(app.view.len(), 1);
        assert_eq!(app.packages[app.view[0]].id, "firefox");
        app.search.set_text("nie-ma-takiego");
        app.on_query_changed();
        assert!(app.view.is_empty());
        assert!(app.status_page().is_some());
    }

    #[test]
    fn featured_orders_by_config() {
        let app = test_app();
        assert_eq!(app.page, Page::Featured);
        assert_eq!(app.view.len(), 1);
        assert_eq!(app.packages[app.view[0]].id, "firefox");
    }

    #[test]
    fn search_is_global_not_limited_to_current_page() {
        let mut app = test_app();
        // Strona „Polecane” zawiera tylko Firefoksa, a GIMP ma się znaleźć.
        assert_eq!(app.view.len(), 1);
        app.search.set_text("gimp");
        app.on_query_changed();
        assert_eq!(app.view.len(), 1);
        assert_eq!(app.packages[app.view[0]].id, "gimp");

        // Opis też jest przeszukiwany.
        app.search.set_text("image editor");
        app.on_query_changed();
        assert_eq!(app.packages[app.view[0]].id, "gimp");
    }

    #[test]
    fn navigation_clears_the_query() {
        let mut app = test_app();
        app.search.set_text("gimp");
        app.on_query_changed();
        app.go_to(Page::Installed);
        assert!(app.search.is_empty());
        assert_eq!(app.view.len(), 1);
        assert_eq!(app.packages[app.view[0]].id, "gimp");
    }

    #[test]
    fn opening_details_advances_sheet_animation() {
        let mut app = test_app();
        let key = app.packages[0].key();
        app.open_details(&key);
        assert!(app.sheet.is_open());
        let before = app.sheet.progress();
        app.tick(0.05);
        assert!(
            app.sheet.progress() > before,
            "panel musi się wysunąć po kliknięciu pakietu"
        );
        for _ in 0..30 {
            app.tick(0.05);
        }
        assert!((app.sheet.progress() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn toast_animation_progresses() {
        let mut app = test_app();
        app.show_toast("Authorization requested".to_owned());
        let before = app.toast.progress();
        app.tick(0.05);
        assert!(app.toast.progress() > before);
    }

    #[test]
    fn admin_hint_follows_privilege_setting() {
        let mut app = test_app();
        let key = app.packages[0].key();
        app.open_details(&key);
        // W testach `Privilege::None` — bez okna hasła.
        assert!(!app.action_requires_admin());
        app.config.privilege = Privilege::Pkexec;
        assert!(app.action_requires_admin());
    }

    #[test]
    fn installed_package_requires_confirmation_before_removal() {
        let mut app = test_app();
        let source = format!("test-{}", std::process::id());
        // Bez aktualizacji: pierwszy klik w „Usuń” ma tylko uzbroić potwierdzenie,
        // a nie od razu uruchamiać menedżer pakietów.
        app.updates.insert(source.clone(), HashMap::new());
        app.rebuild_source_packages(&source);
        let key = package_key(&source, "gimp");
        app.open_details(&key);
        assert!(!app.confirm_remove);
        app.detail_primary();
        assert!(app.confirm_remove);
        assert!(app.working.is_none());
    }

    #[test]
    fn navigation_history_supports_back_and_forward() {
        let mut app = test_app();
        assert!(!app.can_go_back());
        assert!(!app.can_go_forward());
        app.go_to(Page::All);
        app.go_to(Page::Installed);
        assert!(app.can_go_back());
        app.go_back();
        assert_eq!(app.page, Page::All);
        app.go_back();
        assert_eq!(app.page, Page::Featured);
        assert!(!app.can_go_back());
        assert!(app.can_go_forward());
        app.go_forward();
        assert_eq!(app.page, Page::All);
        app.go_forward();
        assert_eq!(app.page, Page::Installed);
        assert!(!app.can_go_forward());
    }

    #[test]
    fn view_mode_switches_between_grid_and_list() {
        let mut app = test_app();
        assert_eq!(app.view_mode, ViewMode::Grid);
        app.toggle_view_mode();
        assert_eq!(app.view_mode, ViewMode::List);
        app.toggle_view_mode();
        assert_eq!(app.view_mode, ViewMode::Grid);
    }

    #[test]
    fn every_source_has_a_nav_entry() {
        let app = test_app();
        assert!(app
            .nav_pages
            .iter()
            .any(|page| matches!(page, Some(Page::Source(_)))));
    }
}

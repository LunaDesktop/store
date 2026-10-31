//! Praca w tle.
//!
//! Każde zapytanie i każda instalacja idzie do osobnego wątku, a wynik wraca
//! przez kanał `mpsc`. UI nigdy nie czeka na menedżer pakietów — dzięki temu
//! animacje i przewijanie pozostają płynne nawet przy `apt-cache search .`
//! albo wolnym repozytorium Flatpaka.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc::{channel, Receiver, Sender};

use crate::backends::{self, OpKind, WorkerSettings};
use crate::config::RepoConfig;
use crate::model::{BackendKind, RawPackage};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionKind {
    Install,
    Remove,
    Update,
    UpdateAll,
}

impl ActionKind {
    pub fn from_op(kind: OpKind) -> Self {
        match kind {
            OpKind::Install => Self::Install,
            OpKind::Remove => Self::Remove,
            OpKind::Update => Self::Update,
            OpKind::UpdateAll => Self::UpdateAll,
        }
    }
}

/// Jedna pozycja instalacji zbiorczej (zaznaczone pakiety).
#[derive(Debug, Clone)]
pub struct BulkAction {
    pub repo: RepoConfig,
    pub kind: ActionKind,
    pub id: String,
    pub name: String,
    pub remote: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Job {
    Catalog(RepoConfig),
    Status(RepoConfig),
    Describe(RepoConfig, String),
    Action {
        repo: RepoConfig,
        kind: ActionKind,
        id: String,
        name: String,
        remote: Option<String>,
    },
    /// Kilka instalacji/aktualizacji wykonywanych po kolei w jednym wątku.
    ///
    /// Sekwencyjnie, bo menedżery pakietów blokują własną bazę — równoległe
    /// wywołania kończyłyby się błędem „database is locked”.
    Bulk(Vec<BulkAction>),
    UpdateAll(Vec<RepoConfig>),
}

#[derive(Debug, Clone)]
pub struct ActionOutcome {
    pub kind: ActionKind,
    pub name: String,
    pub success: bool,
    pub detail: Option<String>,
    pub source: Option<String>,
}

#[derive(Debug)]
pub enum JobResult {
    Catalog {
        source: String,
        packages: Vec<RawPackage>,
        error: Option<String>,
    },
    Status {
        source: String,
        installed: Option<Vec<(String, String)>>,
        updates: Option<Vec<(String, String)>>,
        error: Option<String>,
        warning: Option<String>,
    },
    Described {
        source: String,
        id: String,
        description: String,
    },
    ActionDone(ActionOutcome),
    /// Wyniki instalacji zbiorczej (po jednym na zaznaczony pakiet).
    BulkDone(Vec<ActionOutcome>),
}

impl Job {
    fn source(&self) -> Option<String> {
        match self {
            Job::Catalog(repo) | Job::Status(repo) => Some(repo.id.clone()),
            Job::Describe(repo, _) => Some(repo.id.clone()),
            Job::Action { repo, .. } => Some(repo.id.clone()),
            Job::Bulk(_) | Job::UpdateAll(_) => None,
        }
    }

    fn run(&self, settings: WorkerSettings) -> JobResult {
        match self {
            Job::Catalog(repo) => match backends::catalog(repo) {
                Ok(packages) => JobResult::Catalog {
                    source: repo.id.clone(),
                    packages,
                    error: None,
                },
                Err(error) => JobResult::Catalog {
                    source: repo.id.clone(),
                    packages: Vec::new(),
                    error: Some(error),
                },
            },
            Job::Status(repo) => match backends::status(repo) {
                Ok(status) => JobResult::Status {
                    source: repo.id.clone(),
                    installed: Some(status.installed),
                    updates: Some(status.updates),
                    error: None,
                    warning: status.warning,
                },
                Err(error) => JobResult::Status {
                    source: repo.id.clone(),
                    installed: None,
                    updates: None,
                    error: Some(error),
                    warning: None,
                },
            },
            Job::Describe(repo, id) => match backends::describe(repo, id) {
                Ok(description) => JobResult::Described {
                    source: repo.id.clone(),
                    id: id.clone(),
                    description,
                },
                // Opis jest opcjonalny — błąd nie może zamienić opisu w komunikat.
                Err(_) => JobResult::Described {
                    source: repo.id.clone(),
                    id: id.clone(),
                    description: String::new(),
                },
            },
            Job::Action {
                repo,
                kind,
                id,
                name,
                remote,
            } => {
                let op = match kind {
                    ActionKind::Install => OpKind::Install,
                    ActionKind::Remove => OpKind::Remove,
                    ActionKind::Update => OpKind::Update,
                    ActionKind::UpdateAll => OpKind::UpdateAll,
                };
                let result = backends::operations(repo, op, id, remote.as_deref())
                    .and_then(|operations| run_operations(settings, &operations));
                outcome(*kind, name.clone(), result, Some(repo.id.clone()))
            }
            Job::Bulk(items) => {
                let mut outcomes = Vec::with_capacity(items.len());
                for item in items {
                    let op = match item.kind {
                        ActionKind::Install => OpKind::Install,
                        ActionKind::Remove => OpKind::Remove,
                        ActionKind::Update => OpKind::Update,
                        ActionKind::UpdateAll => OpKind::UpdateAll,
                    };
                    let result =
                        backends::operations(&item.repo, op, &item.id, item.remote.as_deref())
                            .and_then(|operations| run_operations(settings.clone(), &operations));
                    let outcome = match result {
                        Ok(log) => ActionOutcome {
                            kind: item.kind,
                            name: item.name.clone(),
                            success: true,
                            detail: (!log.is_empty()).then_some(log),
                            source: Some(item.repo.id.clone()),
                        },
                        Err(error) => ActionOutcome {
                            kind: item.kind,
                            name: item.name.clone(),
                            success: false,
                            detail: Some(error),
                            source: Some(item.repo.id.clone()),
                        },
                    };
                    // Błędne hasło dotyczy całej serii — nie ma sensu wywoływać
                    // kolejnych menedżerów z tym samym (złym) hasłem.
                    let auth_error = outcome
                        .detail
                        .as_deref()
                        .map(|detail| {
                            let lower = detail.to_lowercase();
                            lower.contains("sorry, try again")
                                || lower.contains("incorrect password")
                                || lower.contains("authentication failure")
                        })
                        .unwrap_or(false);
                    outcomes.push(outcome);
                    if auth_error {
                        break;
                    }
                }
                JobResult::BulkDone(outcomes)
            }
            Job::UpdateAll(repos) => {
                let mut updated = 0usize;
                let mut errors = Vec::new();
                let mut names = Vec::new();
                for repo in repos {
                    match backends::operations(repo, OpKind::UpdateAll, "", None)
                        .and_then(|operations| run_operations(settings.clone(), &operations))
                    {
                        Ok(_) => updated += 1,
                        Err(error) => {
                            errors.push(format!("{}: {error}", repo.id));
                            break;
                        }
                    }
                    names.push(repo.name.clone());
                }
                if errors.is_empty() {
                    JobResult::ActionDone(ActionOutcome {
                        kind: ActionKind::UpdateAll,
                        name: names.join(", "),
                        success: true,
                        detail: (!names.is_empty()).then(|| names.join(", ")),
                        source: None,
                    })
                } else {
                    JobResult::ActionDone(ActionOutcome {
                        kind: ActionKind::UpdateAll,
                        name: format!("{updated}"),
                        success: false,
                        detail: Some(errors.join("\n")),
                        source: None,
                    })
                }
            }
        }
    }

    fn panic_result(&self) -> JobResult {
        match self {
            Job::Catalog(repo) => JobResult::Catalog {
                source: repo.id.clone(),
                packages: Vec::new(),
                error: Some("wątek backendu został przerwany".to_owned()),
            },
            Job::Status(repo) => JobResult::Status {
                source: repo.id.clone(),
                installed: None,
                updates: None,
                error: Some("wątek backendu został przerwany".to_owned()),
                warning: None,
            },
            Job::Describe(repo, id) => JobResult::Described {
                source: repo.id.clone(),
                id: id.clone(),
                description: String::new(),
            },
            Job::Action {
                repo, kind, name, ..
            } => JobResult::ActionDone(ActionOutcome {
                kind: *kind,
                name: name.clone(),
                success: false,
                detail: Some("wątek backendu został przerwany".to_owned()),
                source: Some(repo.id.clone()),
            }),
            Job::UpdateAll(_) => JobResult::ActionDone(ActionOutcome {
                kind: ActionKind::UpdateAll,
                name: String::new(),
                success: false,
                detail: Some("wątek backendu został przerwany".to_owned()),
                source: None,
            }),
            Job::Bulk(items) => JobResult::BulkDone(
                items
                    .iter()
                    .map(|item| ActionOutcome {
                        kind: item.kind,
                        name: item.name.clone(),
                        success: false,
                        detail: Some("wątek backendu został przerwany".to_owned()),
                        source: Some(item.repo.id.clone()),
                    })
                    .collect(),
            ),
        }
    }
}

/// Wykonuje operacje przez wspólny dziennik, jeśli cache jest dostępny.
fn run_operations(
    settings: WorkerSettings,
    operations: &[backends::Operation],
) -> Result<String, String> {
    match crate::oplog::log_path() {
        Some(path) => backends::execute_logged(settings, operations, &path),
        None => backends::execute(settings, operations),
    }
}

fn outcome(
    kind: ActionKind,
    name: String,
    result: Result<String, String>,
    source: Option<String>,
) -> JobResult {
    match result {
        Ok(log) => JobResult::ActionDone(ActionOutcome {
            kind,
            name,
            success: true,
            detail: (!log.is_empty()).then_some(log),
            source,
        }),
        Err(error) => JobResult::ActionDone(ActionOutcome {
            kind,
            name,
            success: false,
            detail: Some(error),
            source,
        }),
    }
}

/// Kolejka zadań i odbiornik wyników.
pub struct JobRunner {
    tx: Sender<JobResult>,
    rx: Receiver<JobResult>,
    settings: WorkerSettings,
}

impl JobRunner {
    pub fn new(privilege: crate::config::Privilege) -> Self {
        let (tx, rx) = channel();
        Self {
            tx,
            rx,
            settings: WorkerSettings::new(privilege),
        }
    }

    pub fn spawn(&self, job: Job) {
        self.spawn_with(self.settings.clone(), job);
    }

    /// Uruchamia zadanie z hasłem administratora podanym w aplikacji.
    ///
    /// Hasło trafia tylko do `sudo -S` w wątku roboczym; nie jest zapisywane
    /// w dzienniku ani w argumentach polecenia.
    pub fn spawn_with_password(&self, job: Job, password: Option<String>) {
        self.spawn_with(self.settings.clone().with_password(password), job);
    }

    fn spawn_with(&self, settings: WorkerSettings, job: Job) {
        let tx = self.tx.clone();
        let fallback = job.clone();
        let if_unspawned = job.clone();
        let spawned = std::thread::Builder::new()
            .name(format!(
                "luna-store-{}",
                job.source().unwrap_or_else(|| "all".to_owned())
            ))
            .spawn(move || {
                let result = catch_unwind(AssertUnwindSafe(|| job.run(settings)))
                    .unwrap_or_else(|_| fallback.panic_result());
                let _ = tx.send(result);
            });
        if spawned.is_err() {
            // Bez tego licznik `loading` zostałby w stanie „trwa” na zawsze.
            let _ = self.tx.send(if_unspawned.panic_result());
        }
    }

    pub fn poll(&self) -> Vec<JobResult> {
        self.rx.try_iter().collect()
    }
}

/// Backend źródła (dla UI/pośrednich operacji).
pub fn backend(repo: &RepoConfig) -> BackendKind {
    repo.backend
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backends::valid_package;
    use crate::config::{Commands, Enabled};

    fn repo(id: &str, backend: BackendKind) -> RepoConfig {
        RepoConfig {
            id: id.into(),
            name: id.into(),
            backend,
            enabled: Enabled::Yes,
            remote: None,
            installation: None,
            probe: None,
            overrides: Commands::default(),
        }
    }

    #[test]
    fn action_kind_maps_from_operations() {
        assert_eq!(ActionKind::from_op(OpKind::Install), ActionKind::Install);
        assert_eq!(ActionKind::from_op(OpKind::Remove), ActionKind::Remove);
        assert_eq!(ActionKind::from_op(OpKind::Update), ActionKind::Update);
        assert_eq!(
            ActionKind::from_op(OpKind::UpdateAll),
            ActionKind::UpdateAll
        );
    }

    #[test]
    fn job_source_is_used_for_thread_name() {
        let job = Job::Catalog(repo("arch", BackendKind::Pacman));
        assert_eq!(job.source().as_deref(), Some("arch"));
    }

    #[test]
    fn package_validation_is_shared() {
        assert!(valid_package("firefox"));
        assert!(!valid_package("-rf"));
        assert!(!valid_package("a b"));
    }
}

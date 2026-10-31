//! # luna-store
//!
//! Sklep z oprogramowaniem środowiska Luna, zbudowany wyłącznie na **libLunie**.
//!
//! - **Katalogi z pliku** — repozytoria i polecane aplikacje opisuje
//!   `repositories.conf` ([`config`]).
//! - **Backendy** — pacman, pag, apt, dnf, emerge, Flatpak oraz `custom`
//!   ([`backends`]).
//! - **Szybkość** — katalogi są cache'owane ([`cache`]), a wszystkie wolne
//!   operacje idą do wątków ([`worker`]).
//! - **UI** — lista wirtualizowana, karty aplikacji, panel szczegółów
//!   ([`ui`]).
//!
//! ```no_run
//! luna_store::run().unwrap();
//! ```

pub mod admin;
pub mod app;
pub mod backends;
pub mod cache;
pub mod config;
pub mod model;
pub mod notify;
pub mod oplog;
pub mod ui;
pub mod worker;

pub use app::run;

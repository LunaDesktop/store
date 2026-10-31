//! Renderuje Luna Store do PNG — podgląd bez kompozytora Wayland.
//!
//! Użycie:
//!   `cargo run --bin store-render -- [--dark] [--lang CODE] [--page all|installed|updates] [--details] [--menu] [--list] [--search QUERY] [wyjście.png]`
//!
//! Dane pochodzą z aktywnych źródeł (tak jak w działającej aplikacji), ale
//! pobierane są synchronicznie — to narzędzie deweloperskie, nie ścieżka UI.

use std::error::Error;

use libluna_core::{Accent, Size, ThemePreference};
use libluna_i18n::{set_locale, Locale};
use libluna_render::{Scene, SoftwareRenderer, TextEngine};
use libluna_widgets::ViewMode;
use luna_store::app::{App, Page};
use luna_store::backends;
use luna_store::config::StoreConfig;
use luna_store::ui;

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut dark = false;
    let mut lang = "pl".to_owned();
    let mut page = Page::Featured;
    let mut details = false;
    let mut menu = false;
    let mut list = false;
    let mut search: Option<String> = None;
    let mut out: Option<String> = None;

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--dark" => {
                dark = true;
                index += 1;
            }
            "--lang" => {
                if let Some(value) = args.get(index + 1) {
                    lang = value.clone();
                }
                index += 2;
            }
            "--details" => {
                details = true;
                index += 1;
            }
            "--menu" => {
                menu = true;
                index += 1;
            }
            "--list" => {
                list = true;
                index += 1;
            }
            "--search" => {
                search = args.get(index + 1).cloned();
                index += 2;
            }
            "--page" => {
                page = match args.get(index + 1).map(String::as_str) {
                    Some("all") => Page::All,
                    Some("installed") => Page::Installed,
                    Some("updates") => Page::Updates,
                    Some("featured") | None => Page::Featured,
                    Some(source) => Page::Source(source.trim_start_matches("source:").to_owned()),
                };
                index += 2;
            }
            other => {
                out = Some(other.to_owned());
                index += 1;
            }
        }
    }

    let out = out.unwrap_or_else(|| {
        if dark {
            "luna-store-dark.png".to_owned()
        } else {
            "luna-store.png".to_owned()
        }
    });

    set_locale(Locale::new(&lang));
    libluna_i18n::load_catalogs_from_dirs();

    let preference = if dark {
        ThemePreference::Dark
    } else {
        ThemePreference::Light
    };
    let theme = preference.resolve(false, Accent::Blue);

    let mut app = App::from_config(StoreConfig::default());
    let repos = app
        .repos
        .iter()
        .map(|repo| repo.cfg.clone())
        .collect::<Vec<_>>();
    for repo in repos {
        let Ok(packages) = backends::catalog(&repo) else {
            continue;
        };
        let (installed, updates) = backends::status(&repo)
            .map(|status| (status.installed, status.updates))
            .unwrap_or_default();
        app.preview_data(&repo.id, packages, installed, updates);
        app.go_to(page.clone());
    }
    app.go_to(page.clone());
    if let Some(query) = &search {
        app.search.set_text(query.clone());
        app.on_query_changed();
    }
    if details {
        if let Some(&index) = app.view.first() {
            let key = app.packages[index].key();
            app.open_details(&key);
            app.sheet.set_open(true);
        }
    }
    if menu {
        app.menu_open = true;
    }
    if list {
        app.view_mode = ViewMode::List;
    }

    let size = Size::new(1120.0, 760.0);
    let text = TextEngine::system();
    let mut scene = Scene::new();
    ui::paint(&app, &mut scene, size, &theme, &text);

    let mut renderer = SoftwareRenderer::new(1120, 760);
    renderer.draw(&scene, 1.0, theme.colors.window_bg);
    let png = renderer.encode_png().ok_or("failed to encode PNG")?;
    std::fs::write(&out, png)?;
    println!("saved {out} (1120x760, lang={lang}, page={:?})", page);
    Ok(())
}

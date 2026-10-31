//! Rysowanie i obsługa wejścia Luna Store.
//!
//! Układ liczymy w jednym miejscu ([`layout`]) i używamy go zarówno do rysowania,
//! jak i do trafień — dzięki temu przyciski na kartach nie „rozjeżdżają się”
//! względem tego, co widać. Lista jest wirtualizowana: rysujemy tylko widoczne
//! wiersze, więc nawet katalog `apt` ze 100 000 pakietów przewija się płynnie.

use libluna_core::style::weight;
use libluna_core::{
    ButtonState, Color, Event, Insets, KeyEvent, Point, PointerButton, Rect, Size, Theme,
};
use libluna_i18n::tr;
use libluna_render::{Scene, TextEngine};
use libluna_widgets::{
    clipboard_command, paint_icon, Button, ButtonVariant, ControlsSide, HeaderBar, PaintCtx,
    Spinner, TextClipboardCommand, ViewMode, WindowControl,
};

use crate::app::{source_icon, App, Page};
use crate::model::Package;

const SIDEBAR_W: f64 = 236.0;
const CONTENT_PAD: f64 = 28.0;
const HEADER_BTN: f64 = 30.0;
const HEADER_SEARCH_H: f64 = 30.0;
const UPDATE_ALL_W: f64 = 132.0;
const CARD_H: f64 = 132.0;
const CARD_GAP: f64 = 14.0;
const MIN_CARD_W: f64 = 260.0;
const LIST_ROW_H: f64 = 66.0;
const LIST_GAP: f64 = 8.0;
const SCROLL_STEP: f64 = 64.0;
const SCROLL_UNIT: f64 = 20.0;

/// Geometria okna.
#[derive(Debug, Clone, Copy)]
pub struct Layout {
    pub header: Rect,
    pub sidebar: Rect,
    pub nav: Rect,
    pub content: Rect,
    pub title: Rect,
    /// Przełącznik paska bocznego w headerbarze.
    pub sidebar_toggle: Rect,
    /// Historia: wstecz / dalej.
    pub back: Rect,
    pub forward: Rect,
    /// Pole wyszukiwania w headerbarze.
    pub search: Rect,
    /// Odświeżanie w headerbarze.
    pub refresh: Rect,
    /// Siatka / lista.
    pub view: Rect,
    /// Menu ☰ w headerbarze.
    pub menu: Rect,
    pub update_all: Rect,
    pub status: Rect,
    pub viewport: Rect,
}

pub fn layout(size: Size, theme: &Theme, sidebar_visible: bool) -> Layout {
    let header = Rect::from_xywh(0.0, 0.0, size.width, theme.metrics.headerbar.height);
    let sidebar_w = if sidebar_visible {
        SIDEBAR_W.min((size.width * 0.34).max(200.0))
    } else {
        0.0
    };
    let sidebar = Rect::from_xywh(
        0.0,
        header.bottom(),
        sidebar_w,
        (size.height - header.height()).max(0.0),
    );
    let nav = Rect::from_xywh(
        sidebar.x() + 8.0,
        sidebar.y() + 8.0,
        (sidebar.width() - 16.0).max(0.0),
        (sidebar.height() - 42.0).max(0.0),
    );
    let content = Rect::from_xywh(
        sidebar.right(),
        header.bottom(),
        (size.width - sidebar.width()).max(0.0),
        (size.height - header.height()).max(0.0),
    );
    let pad = CONTENT_PAD.min((content.width() * 0.07).max(16.0));
    let title = Rect::from_xywh(
        content.x() + pad,
        content.y() + 18.0,
        (content.width() - 2.0 * pad).max(0.0),
        52.0,
    );
    let update_all = Rect::from_xywh(
        (title.right() - UPDATE_ALL_W).max(title.x()),
        title.y() + 8.0,
        UPDATE_ALL_W,
        34.0,
    );
    let status = Rect::from_xywh(title.x(), title.bottom() + 2.0, title.width(), 20.0);
    let viewport = Rect::from_xywh(
        title.x(),
        status.bottom() + 6.0,
        title.width(),
        (content.bottom() - 16.0 - (status.bottom() + 6.0)).max(0.0),
    );

    // Pasek tytułu w stylu Luna Files: kontrolki po prawej, treść między nimi.
    let header_content = HeaderBar::new()
        .controls(ControlsSide::Right)
        .controls_layout(header, theme)
        .content;
    let cy = header.center().y;
    let sidebar_toggle = Rect::from_xywh(
        header_content.x(),
        cy - HEADER_BTN / 2.0,
        HEADER_BTN,
        HEADER_BTN,
    );
    let back = Rect::from_xywh(
        sidebar_toggle.right() + 2.0,
        cy - HEADER_BTN / 2.0,
        HEADER_BTN,
        HEADER_BTN,
    );
    let forward = Rect::from_xywh(
        back.right() + 2.0,
        cy - HEADER_BTN / 2.0,
        HEADER_BTN,
        HEADER_BTN,
    );
    let menu = Rect::from_xywh(
        header_content.right() - HEADER_BTN,
        cy - HEADER_BTN / 2.0,
        HEADER_BTN,
        HEADER_BTN,
    );
    let view = Rect::from_xywh(
        menu.x() - HEADER_BTN - 2.0,
        cy - HEADER_BTN / 2.0,
        HEADER_BTN,
        HEADER_BTN,
    );
    let refresh = Rect::from_xywh(
        view.x() - HEADER_BTN - 2.0,
        cy - HEADER_BTN / 2.0,
        HEADER_BTN,
        HEADER_BTN,
    );
    let search_x = forward.right() + 8.0;
    let search = Rect::from_xywh(
        search_x,
        cy - HEADER_SEARCH_H / 2.0,
        (refresh.x() - 8.0 - search_x).max(40.0),
        HEADER_SEARCH_H,
    );

    Layout {
        header,
        sidebar,
        nav,
        content,
        title,
        sidebar_toggle,
        back,
        forward,
        search,
        refresh,
        view,
        menu,
        update_all,
        status,
        viewport,
    }
}

fn grid_cols(width: f64) -> usize {
    (((width + CARD_GAP) / (MIN_CARD_W + CARD_GAP)).floor() as usize).max(1)
}

fn grid_card_width(viewport_w: f64) -> f64 {
    let cols = grid_cols(viewport_w);
    ((viewport_w - CARD_GAP * (cols.saturating_sub(1)) as f64) / cols as f64).max(120.0)
}

fn item_cols(app: &App, viewport_w: f64) -> usize {
    match app.view_mode {
        ViewMode::Grid => grid_cols(viewport_w),
        ViewMode::List => 1,
    }
}

fn item_height(app: &App) -> f64 {
    match app.view_mode {
        ViewMode::Grid => CARD_H,
        ViewMode::List => LIST_ROW_H,
    }
}

fn item_gap(app: &App) -> f64 {
    match app.view_mode {
        ViewMode::Grid => CARD_GAP,
        ViewMode::List => LIST_GAP,
    }
}

fn item_width(app: &App, viewport_w: f64) -> f64 {
    match app.view_mode {
        ViewMode::Grid => grid_card_width(viewport_w),
        ViewMode::List => viewport_w,
    }
}

fn item_rect(app: &App, lay: &Layout, view_index: usize, offset: f64) -> Rect {
    let cols = item_cols(app, lay.viewport.width());
    let width = item_width(app, lay.viewport.width());
    let gap = item_gap(app);
    let height = item_height(app);
    let row = view_index / cols;
    let col = view_index % cols;
    Rect::from_xywh(
        lay.viewport.x() + col as f64 * (width + gap),
        lay.viewport.y() - offset + row as f64 * (height + gap),
        width,
        height,
    )
}

fn items_content_height(app: &App, view_len: usize, lay: &Layout) -> f64 {
    let cols = item_cols(app, lay.viewport.width());
    let rows = view_len.div_ceil(cols);
    if rows == 0 {
        0.0
    } else {
        let height = item_height(app);
        rows as f64 * (height + item_gap(app)) - item_gap(app)
    }
}

fn item_at(app: &App, lay: &Layout, point: Point) -> Option<usize> {
    if !lay.viewport.contains(point) {
        return None;
    }
    let offset = app.scroll.offset().min(app.scroll.max_offset(lay.viewport));
    let cols = item_cols(app, lay.viewport.width());
    let row_h = item_height(app) + item_gap(app);
    let row = ((point.y - lay.viewport.y() + offset) / row_h).floor() as usize;
    let start = row.saturating_mul(cols);
    let end = (start + cols).min(app.view.len());
    (start..end).find(|index| item_rect(app, lay, *index, offset).contains(point))
}

fn card_action_rect(card: Rect, theme: &Theme, text: &TextEngine, label: &str) -> Rect {
    let button = Button::new(label);
    let width = button.width(text, theme).max(74.0);
    let height = theme.metrics.button.height.min(36.0);
    Rect::from_xywh(
        card.right() - 14.0 - width,
        card.bottom() - 14.0 - height,
        width,
        height,
    )
}

fn card_label(package: &Package) -> String {
    if package.has_update() {
        tr("Update")
    } else if package.is_installed() {
        tr("Details")
    } else {
        tr("Install")
    }
}

fn card_variant(package: &Package) -> ButtonVariant {
    if package.has_update() || !package.is_installed() {
        ButtonVariant::Suggested
    } else {
        ButtonVariant::Default
    }
}

fn page_title(app: &App) -> String {
    if !app.search.text().trim().is_empty() {
        return tr("Search results");
    }
    match &app.page {
        Page::Featured => tr("Featured"),
        Page::All => tr("All apps"),
        Page::Installed => tr("Installed"),
        Page::Updates => tr("Updates"),
        Page::Source(source) => app.repo_name(source).to_owned(),
    }
}

fn page_subtitle(app: &App) -> String {
    if !app.search.text().trim().is_empty() {
        return format!("{} {}", app.view.len(), tr("packages"));
    }
    match &app.page {
        Page::Featured => tr("Curated picks from your sources"),
        Page::All => format!("{} {}", app.view.len(), tr("packages")),
        Page::Installed => format!("{} {}", app.view.len(), tr("installed packages")),
        Page::Updates => {
            let count = app.update_count();
            if count == 0 {
                tr("Everything is up to date")
            } else {
                format!("{count} {}", tr("updates available"))
            }
        }
        Page::Source(source) => {
            let backend = app
                .repos
                .iter()
                .find(|repo| repo.cfg.id == *source)
                .map(|repo| repo.cfg.backend.as_str())
                .unwrap_or("");
            format!("{} · {}", backend, app.view.len())
        }
    }
}

fn page_icon(app: &App) -> libluna_widgets::IconName {
    if !app.search.text().trim().is_empty() {
        return libluna_widgets::IconName::Search;
    }
    match &app.page {
        Page::Featured => libluna_widgets::IconName::Star,
        Page::All => libluna_widgets::IconName::Store,
        Page::Installed => libluna_widgets::IconName::Check,
        Page::Updates => libluna_widgets::IconName::Clock,
        Page::Source(source) => app
            .repos
            .iter()
            .find(|repo| repo.cfg.id == *source)
            .map(|repo| source_icon(repo.cfg.backend))
            .unwrap_or(libluna_widgets::IconName::Archive),
    }
}

pub fn paint(app: &App, scene: &mut Scene, size: Size, theme: &Theme, text: &TextEngine) {
    let lay = layout(size, theme, app.sidebar_visible);
    let mut ctx = PaintCtx::new(scene, theme, text);
    let colors = theme.colors;
    let caption = theme.typography.caption;

    ctx.scene.fill_rect(Rect::from_size(size), colors.window_bg);

    // Sidebar + nawigacja.
    if app.sidebar_visible {
        ctx.scene.fill_rect(lay.sidebar, colors.headerbar_bg);
        ctx.scene.fill_rect(
            Rect::from_xywh(
                lay.sidebar.right() - 1.0,
                lay.sidebar.y(),
                1.0,
                lay.sidebar.height(),
            ),
            colors.border,
        );
        ctx.scene.push_clip(lay.nav);
        app.nav.paint(&mut ctx, lay.nav);
        ctx.scene.pop_clip();
        ctx.label_left(
            lay.sidebar.x() + 16.0,
            lay.sidebar.bottom() - 14.0,
            &format!("Luna Store {}", env!("CARGO_PKG_VERSION")),
            caption.size,
            weight::NORMAL,
            colors.dim_text,
        );
    }

    // Nagłówek okna (CSD) — styl Luna Files: kontrolki i treść w jednym wierszu.
    HeaderBar::new()
        .controls(ControlsSide::Right)
        .paint(&mut ctx, lay.header, app.hover_control);

    paint_header_button(&mut ctx, lay.sidebar_toggle, app.hover_sidebar, false);
    paint_sidebar_icon(
        &mut ctx,
        lay.sidebar_toggle,
        colors.text,
        app.sidebar_visible,
    );

    let back_enabled = app.can_go_back();
    if back_enabled {
        paint_header_button(&mut ctx, lay.back, app.hover_back, false);
    }
    paint_icon(
        &mut ctx,
        libluna_widgets::IconName::ChevronLeft,
        lay.back.deflate(Insets::splat(7.0)),
        if back_enabled {
            colors.text
        } else {
            colors.dim_text.scale_alpha(0.45)
        },
    );
    let forward_enabled = app.can_go_forward();
    if forward_enabled {
        paint_header_button(&mut ctx, lay.forward, app.hover_forward, false);
    }
    paint_icon(
        &mut ctx,
        libluna_widgets::IconName::ChevronRight,
        lay.forward.deflate(Insets::splat(7.0)),
        if forward_enabled {
            colors.text
        } else {
            colors.dim_text.scale_alpha(0.45)
        },
    );

    app.search.paint(&mut ctx, lay.search);

    paint_header_button(&mut ctx, lay.refresh, app.hover_refresh, false);
    let refresh_color = if app.working.is_some() {
        colors.dim_text.scale_alpha(0.5)
    } else {
        colors.text
    };
    paint_refresh_icon(&mut ctx, lay.refresh, refresh_color);

    paint_header_button(&mut ctx, lay.view, app.hover_view, false);
    // Ikona pokazuje bieżący tryb; kliknięcie przełącza siatkę/listę.
    let view_icon = match app.view_mode {
        libluna_widgets::ViewMode::Grid => libluna_widgets::IconName::Grid,
        libluna_widgets::ViewMode::List => libluna_widgets::IconName::List,
    };
    paint_icon(
        &mut ctx,
        view_icon,
        lay.view.deflate(Insets::splat(6.0)),
        colors.text,
    );

    paint_header_button(&mut ctx, lay.menu, app.hover_menu, app.menu_open);
    paint_icon(
        &mut ctx,
        libluna_widgets::IconName::Menu,
        lay.menu.deflate(Insets::splat(6.0)),
        colors.text,
    );

    // Tytuł strony.
    let icon = Rect::from_xywh(lay.title.x(), lay.title.y() + 9.0, 22.0, 22.0);
    paint_icon(&mut ctx, page_icon(app), icon, colors.accent);
    let updates = app.update_count();
    let title_space = if app.page == Page::Updates && updates > 0 {
        lay.title.width() - UPDATE_ALL_W - 12.0
    } else {
        lay.title.width() - 32.0
    };
    let title = page_title(app);
    let title = elide(
        &ctx,
        &title,
        title_space.max(40.0),
        theme.typography.heading.size,
        weight::BOLD,
    );
    ctx.label_left(
        lay.title.x() + 32.0,
        lay.title.y() + 20.0,
        &title,
        theme.typography.heading.size,
        weight::BOLD,
        colors.text,
    );
    let subtitle = elide(
        &ctx,
        &page_subtitle(app),
        (lay.title.width() - 32.0).max(40.0),
        caption.size,
        weight::NORMAL,
    );
    ctx.label_left(
        lay.title.x() + 32.0,
        lay.title.y() + 42.0,
        &subtitle,
        caption.size,
        weight::NORMAL,
        colors.dim_text,
    );

    // „Aktualizuj wszystko” tylko tam, gdzie ma sens.
    if app.page == Page::Updates && updates > 0 {
        let mut button = Button::new(tr("Update all"))
            .variant(ButtonVariant::Suggested)
            .enabled(app.working.is_none());
        button.set_hovered(app.hover_update_all);
        button.paint(&mut ctx, lay.update_all);
    }

    paint_status_line(app, &mut ctx, &lay);

    if let Some(page) = app.status_page() {
        page.paint(&mut ctx, lay.viewport);
    } else {
        paint_items(app, &mut ctx, &lay);
    }

    if app.sheet.is_open() || app.sheet.progress() > 0.001 {
        app.sheet.paint(&mut ctx, size);
        paint_details(app, &mut ctx, size);
    }
    if let Some(panel) = menu_panel(app, &lay, size, theme, text) {
        app.menu.paint(&mut ctx, panel);
    }
    app.toast.paint(&mut ctx, size);
}

fn paint_header_button(ctx: &mut PaintCtx, rect: Rect, hover: bool, active: bool) {
    if hover || active {
        ctx.scene
            .fill_rounded_rect(rect, ctx.theme.radius.small, ctx.theme.colors.shade);
    }
}

/// Ikona przełącznika paska bocznego (◧ / ▭) — spójna z Luna Files.
fn paint_sidebar_icon(ctx: &mut PaintCtx, rect: Rect, color: Color, filled: bool) {
    let c = rect.center();
    let (w, h) = (18.0, 14.0);
    let outer = Rect::from_xywh(c.x - w / 2.0, c.y - h / 2.0, w, h);
    ctx.scene.stroke_rounded_rect(outer, 3.0, 1.5, color);
    if filled {
        let inner = Rect::from_xywh(outer.x() + 2.0, outer.y() + 2.0, w * 0.36, h - 4.0);
        ctx.scene.fill_rounded_rect(inner, 2.0, color);
    }
}

/// Ikona odświeżania (okrągła strzałka) rysowana wektorowo.
fn paint_refresh_icon(ctx: &mut PaintCtx, rect: Rect, color: Color) {
    let center = rect.center();
    let radius = rect.width() * 0.31;
    let steps = 22;
    let mut points = Vec::with_capacity(steps + 1);
    for index in 0..=steps {
        let angle = (40.0 + index as f64 * (280.0 / steps as f64)).to_radians();
        points.push(Point::new(
            center.x + radius * angle.cos(),
            center.y + radius * angle.sin(),
        ));
    }
    ctx.scene.stroke_polyline(&points, 1.8, color);

    // Grocik na początku łuku.
    let tip = points[0];
    let next = points[1];
    let angle = (next.y - tip.y).atan2(next.x - tip.x);
    for offset in [-0.7_f64, 0.7] {
        let dir = angle + offset;
        ctx.scene.stroke_line(
            tip,
            Point::new(tip.x - 5.0 * dir.cos(), tip.y - 5.0 * dir.sin()),
            1.8,
            color,
        );
    }
}

fn menu_panel(
    app: &App,
    lay: &Layout,
    size: Size,
    theme: &Theme,
    text: &TextEngine,
) -> Option<Rect> {
    if !app.menu_open {
        return None;
    }
    let width = app.menu.width(text, theme);
    Some(
        app.menu
            .place(lay.menu, width, Rect::from_size(size), theme),
    )
}

fn paint_status_line(app: &App, ctx: &mut PaintCtx, lay: &Layout) {
    let theme = *ctx.theme;
    let caption = theme.typography.caption;
    let busy = app.working.is_some() || app.is_loading();
    let mut x = lay.status.x();
    if busy {
        let mut spinner = Spinner::new();
        spinner.set_progress(app.spinner);
        spinner.paint(
            ctx,
            Rect::from_xywh(lay.status.x(), lay.status.center().y - 9.0, 18.0, 18.0),
        );
        x += 26.0;
    }

    let (message, color) = if let Some(working) = &app.working {
        (working.clone(), theme.colors.text)
    } else if app.is_loading() {
        (tr("Loading…"), theme.colors.dim_text)
    } else if let Some(warning) = &app.config.warning {
        (
            format!("{}: {warning}", tr("Configuration error")),
            theme.colors.warning,
        )
    } else if let Some(error) = app.error_text() {
        (
            format!("{}: {error}", tr("Source error")),
            theme.colors.warning,
        )
    } else {
        (
            format!(
                "{}: {} · {}: {}",
                tr("Packages"),
                app.view.len(),
                tr("Sources"),
                app.repos.len()
            ),
            theme.colors.dim_text,
        )
    };
    let message = elide(
        ctx,
        &message,
        (lay.status.right() - x).max(40.0),
        caption.size,
        weight::NORMAL,
    );
    ctx.label_left(
        x,
        lay.status.center().y,
        &message,
        caption.size,
        weight::NORMAL,
        color,
    );
}

fn paint_items(app: &App, ctx: &mut PaintCtx, lay: &Layout) {
    match app.view_mode {
        ViewMode::Grid => paint_grid(app, ctx, lay),
        ViewMode::List => paint_list(app, ctx, lay),
    }
}

fn paint_grid(app: &App, ctx: &mut PaintCtx, lay: &Layout) {
    let theme = *ctx.theme;
    let cols = item_cols(app, lay.viewport.width());
    let row_h = item_height(app) + item_gap(app);
    let content_height = items_content_height(app, app.view.len(), lay);
    let mut scroll = app.scroll;
    scroll.set_content_height(content_height);
    let offset = scroll.offset().min(scroll.max_offset(lay.viewport));
    let rows = app.view.len().div_ceil(cols);
    let first_row = (offset / row_h).floor() as usize;
    let last_row = (((offset + lay.viewport.height()) / row_h).ceil() as usize + 1).min(rows);

    ctx.scene.push_clip(lay.viewport);
    for row in first_row..last_row {
        for col in 0..cols {
            let view_index = row * cols + col;
            if view_index >= app.view.len() {
                break;
            }
            let Some(&package_index) = app.view.get(view_index) else {
                continue;
            };
            let package = &app.packages[package_index];
            let card = item_rect(app, lay, view_index, offset);
            paint_card(app, ctx, card, package, view_index, theme);
        }
    }
    ctx.scene.pop_clip();
    scroll.paint_scrollbar(ctx, lay.viewport);
}

fn paint_list(app: &App, ctx: &mut PaintCtx, lay: &Layout) {
    let theme = *ctx.theme;
    let row_h = item_height(app) + item_gap(app);
    let content_height = items_content_height(app, app.view.len(), lay);
    let mut scroll = app.scroll;
    scroll.set_content_height(content_height);
    let offset = scroll.offset().min(scroll.max_offset(lay.viewport));
    let first = (offset / row_h).floor() as usize;
    let last = (((offset + lay.viewport.height()) / row_h).ceil() as usize + 1).min(app.view.len());

    ctx.scene.push_clip(lay.viewport);
    for view_index in first..last {
        let Some(&package_index) = app.view.get(view_index) else {
            continue;
        };
        let rect = item_rect(app, lay, view_index, offset);
        paint_list_row(
            app,
            ctx,
            rect,
            &app.packages[package_index],
            view_index,
            theme,
        );
    }
    ctx.scene.pop_clip();
    scroll.paint_scrollbar(ctx, lay.viewport);
}

fn paint_list_row(
    app: &App,
    ctx: &mut PaintCtx,
    row: Rect,
    package: &Package,
    view_index: usize,
    theme: Theme,
) {
    let colors = theme.colors;
    let body = theme.typography.body;
    let caption = theme.typography.caption;
    let hovered = app.hover_card == Some(view_index);
    ctx.scene.fill_rounded_rect(
        row,
        theme.radius.medium,
        if hovered {
            colors.shade
        } else {
            colors.card_bg
        },
    );
    ctx.scene
        .stroke_rounded_rect(row, theme.radius.medium, 1.0, colors.border);

    let tile = Rect::from_xywh(row.x() + 12.0, row.center().y - 17.0, 34.0, 34.0);
    ctx.scene
        .fill_rounded_rect(tile, theme.radius.small, colors.accent.scale_alpha(0.16));
    paint_icon(
        ctx,
        source_icon(package.backend),
        tile.deflate(Insets::splat(8.0)),
        colors.accent,
    );

    let label = card_label(package);
    let action = card_action_rect(row, &theme, ctx.text, &label);
    let text_x = row.x() + 56.0;
    let text_right = (action.x() - 12.0).max(text_x + 40.0);

    let name = elide(
        ctx,
        &package.name,
        text_right - text_x,
        body.size,
        weight::MEDIUM,
    );
    ctx.label_left(
        text_x,
        row.y() + 22.0,
        &name,
        body.size,
        weight::MEDIUM,
        colors.text,
    );

    let name_width = ctx.measure(&name, body.size, weight::MEDIUM);
    let description_x = text_x + name_width + 10.0;
    if description_x < text_right - 24.0 && !package.description.is_empty() {
        let description = elide(
            ctx,
            &package.description,
            text_right - description_x,
            caption.size,
            weight::NORMAL,
        );
        ctx.label_left(
            description_x,
            row.y() + 22.0,
            &description,
            caption.size,
            weight::NORMAL,
            colors.dim_text,
        );
    }

    let state = package_state(package);
    let meta = if package.is_installed() || package.version.is_empty() {
        format!("{} · {state}", app.repo_name(&package.source))
    } else {
        format!(
            "{} · {} · {state}",
            app.repo_name(&package.source),
            package.version
        )
    };
    let meta = elide(
        ctx,
        &meta,
        text_right - text_x,
        caption.size,
        weight::NORMAL,
    );
    ctx.label_left(
        text_x,
        row.y() + 43.0,
        &meta,
        caption.size,
        weight::NORMAL,
        package_state_color(package, &theme),
    );

    let mut button = Button::new(label)
        .variant(card_variant(package))
        .enabled(app.working.is_none());
    button.set_hovered(hovered && app.hover_action);
    button.paint(ctx, action);
}

fn package_state(package: &Package) -> String {
    if let Some(update) = &package.update_version {
        format!(
            "{} → {}",
            package.installed_version.as_deref().unwrap_or(""),
            update
        )
    } else if let Some(installed) = &package.installed_version {
        format!("{} {installed}", tr("Installed"))
    } else {
        tr("Available")
    }
}

fn package_state_color(package: &Package, theme: &Theme) -> Color {
    if package.has_update() {
        theme.colors.warning
    } else if package.is_installed() {
        theme.colors.success
    } else {
        theme.colors.dim_text
    }
}

fn paint_card(
    app: &App,
    ctx: &mut PaintCtx,
    card: Rect,
    package: &Package,
    view_index: usize,
    theme: Theme,
) {
    let colors = theme.colors;
    let body = theme.typography.body;
    let caption = theme.typography.caption;
    let hovered = app.hover_card == Some(view_index);
    ctx.scene.fill_rounded_rect(
        card,
        theme.radius.large,
        if hovered {
            colors.shade
        } else {
            colors.card_bg
        },
    );
    ctx.scene
        .stroke_rounded_rect(card, theme.radius.large, 1.0, colors.border);

    // Ikona źródła.
    let tile = Rect::from_xywh(card.x() + 14.0, card.y() + 14.0, 40.0, 40.0);
    ctx.scene
        .fill_rounded_rect(tile, theme.radius.small, colors.accent.scale_alpha(0.16));
    paint_icon(
        ctx,
        source_icon(package.backend),
        tile.deflate(libluna_core::Insets::symmetric(10.0, 10.0)),
        colors.accent,
    );

    let text_x = card.x() + 66.0;
    let text_w = (card.right() - 14.0 - text_x).max(40.0);
    let name = elide(ctx, &package.name, text_w, body.size, weight::MEDIUM);
    ctx.label_left(
        text_x,
        card.y() + 27.0,
        &name,
        body.size,
        weight::MEDIUM,
        colors.text,
    );

    let mut meta = app.repo_name(&package.source).to_owned();
    if !package.version.is_empty() {
        meta.push_str(" · ");
        meta.push_str(&package.version);
    }
    let meta = elide(ctx, &meta, text_w, caption.size, weight::NORMAL);
    ctx.label_left(
        text_x,
        card.y() + 48.0,
        &meta,
        caption.size,
        weight::NORMAL,
        colors.dim_text,
    );

    let description = if package.description.is_empty() {
        tr("No description")
    } else {
        package.description.clone()
    };
    let description = elide(
        ctx,
        &description,
        (card.width() - 28.0).max(40.0),
        caption.size,
        weight::NORMAL,
    );
    ctx.label_left(
        card.x() + 14.0,
        card.y() + 76.0,
        &description,
        caption.size,
        weight::NORMAL,
        colors.dim_text,
    );

    // Stan: dostępny / zainstalowany / aktualizacja.
    let state = package_state(package);
    let state_color = package_state_color(package, &theme);
    ctx.label_left(
        card.x() + 14.0,
        card.bottom() - 26.0,
        &elide(
            ctx,
            &state,
            (card.width() - 110.0).max(40.0),
            caption.size,
            weight::NORMAL,
        ),
        caption.size,
        weight::NORMAL,
        state_color,
    );

    let label = card_label(package);
    let rect = card_action_rect(card, &theme, ctx.text, &label);
    let mut button = Button::new(label)
        .variant(card_variant(package))
        .enabled(app.working.is_none());
    button.set_hovered(hovered && app.hover_action);
    button.paint(ctx, rect);
}

fn paint_details(app: &App, ctx: &mut PaintCtx, size: Size) {
    let Some(package) = app.selected_package() else {
        return;
    };
    let theme = *ctx.theme;
    let colors = theme.colors;
    let body = theme.typography.body;
    let caption = theme.typography.caption;
    let content = app.sheet.content_rect(size, &theme);
    let pad = 24.0;
    let inner = Rect::from_xywh(
        content.x() + pad,
        content.y() + 14.0,
        (content.width() - 2.0 * pad).max(80.0),
        (content.height() - 28.0).max(40.0),
    );

    ctx.scene.push_clip(content);

    let tile = Rect::from_xywh(inner.x(), inner.y(), 44.0, 44.0);
    ctx.scene
        .fill_rounded_rect(tile, theme.radius.small, colors.accent.scale_alpha(0.16));
    paint_icon(
        ctx,
        source_icon(package.backend),
        tile.deflate(libluna_core::Insets::symmetric(11.0, 11.0)),
        colors.accent,
    );

    let info_x = inner.x() + 56.0;
    let info_w = (inner.right() - info_x).max(50.0);
    let source = format!(
        "{} · {}",
        app.repo_name(&package.source),
        package.backend.as_str()
    );
    let source = elide(ctx, &source, info_w, caption.size, weight::NORMAL);
    ctx.label_left(
        info_x,
        inner.y() + 16.0,
        &source,
        caption.size,
        weight::NORMAL,
        colors.dim_text,
    );
    let id = elide(ctx, &package.id, info_w, caption.size, weight::NORMAL);
    ctx.label_left(
        info_x,
        inner.y() + 34.0,
        &id,
        caption.size,
        weight::NORMAL,
        colors.dim_text,
    );

    let description = if package.description.is_empty() {
        tr("No description")
    } else {
        package.description.clone()
    };
    let mut y = inner.y() + 62.0;
    for line in wrap(
        ctx,
        &description,
        inner.width(),
        body.size,
        weight::NORMAL,
        4,
    ) {
        ctx.label_left(inner.x(), y, &line, body.size, weight::NORMAL, colors.text);
        y += body.line_height;
    }

    y += 8.0;
    let mut version = package.version.clone();
    if let Some(update) = &package.update_version {
        version = if package.installed_version.is_some() {
            format!(
                "{} → {}",
                package.installed_version.as_deref().unwrap_or(""),
                update
            )
        } else {
            update.clone()
        };
    }
    paint_meta(
        ctx,
        inner.x(),
        &mut y,
        inner.width(),
        tr("Version").as_str(),
        &version,
        caption.size,
        theme,
    );

    if let Some(installed) = &package.installed_version {
        paint_meta(
            ctx,
            inner.x(),
            &mut y,
            inner.width(),
            tr("Installed version").as_str(),
            installed,
            caption.size,
            theme,
        );
    }
    if let Some(update) = &package.update_version {
        paint_meta(
            ctx,
            inner.x(),
            &mut y,
            inner.width(),
            tr("Update available").as_str(),
            update,
            caption.size,
            theme,
        );
    }
    paint_meta(
        ctx,
        inner.x(),
        &mut y,
        inner.width(),
        tr("Source").as_str(),
        app.repo_name(&package.source),
        caption.size,
        theme,
    );

    if app.confirm_remove {
        let warning = tr("Remove this package?");
        ctx.label_left(
            inner.x(),
            inner.bottom() - 56.0,
            &warning,
            body.size,
            weight::MEDIUM,
            colors.error,
        );
    }

    if let Some((primary, secondary)) = detail_button_rects(app, size, &theme, ctx.text) {
        if app.action_requires_admin() && !app.confirm_remove {
            // Wolne miejsce po lewej stronie przycisków — informacja, że polkit
            // poprosi o hasło.
            let left = secondary.map_or(primary.x(), |rect| rect.x());
            let hint = elide(
                ctx,
                &tr("Requires administrator approval (polkit)"),
                (left - inner.x() - 12.0).max(40.0),
                caption.size,
                weight::NORMAL,
            );
            ctx.label_left(
                inner.x(),
                primary.center().y,
                &hint,
                caption.size,
                weight::NORMAL,
                colors.dim_text,
            );
        }
        let (primary_label, secondary_label) = detail_labels(app);
        let button = Button::new(primary_label.clone())
            .variant(if primary_label == tr("Remove") {
                ButtonVariant::Destructive
            } else {
                ButtonVariant::Suggested
            })
            .enabled(app.working.is_none());
        button.paint(ctx, primary);
        if let (Some(rect), Some(label)) = (secondary, secondary_label) {
            Button::new(label)
                .variant(ButtonVariant::Default)
                .enabled(app.working.is_none())
                .paint(ctx, rect);
        }
    }

    ctx.scene.pop_clip();
}

#[allow(clippy::too_many_arguments)]
fn paint_meta(
    ctx: &mut PaintCtx,
    x: f64,
    y: &mut f64,
    width: f64,
    label: &str,
    value: &str,
    size: f64,
    theme: Theme,
) {
    ctx.label_left(x, *y, label, size, weight::NORMAL, theme.colors.dim_text);
    let value = elide(ctx, value, (width - 130.0).max(40.0), size, weight::NORMAL);
    ctx.label_left(
        x + 130.0,
        *y,
        &value,
        size,
        weight::NORMAL,
        theme.colors.text,
    );
    *y += size * 1.65;
}

fn detail_labels(app: &App) -> (String, Option<String>) {
    if app.confirm_remove {
        return (tr("Remove"), Some(tr("Cancel")));
    }
    let Some(package) = app.selected_package() else {
        return (tr("Close"), None);
    };
    if package.is_installed() && package.has_update() {
        (tr("Update"), Some(tr("Remove")))
    } else if package.is_installed() {
        (tr("Remove"), None)
    } else {
        (tr("Install"), None)
    }
}

fn detail_button_rects(
    app: &App,
    size: Size,
    theme: &Theme,
    text: &TextEngine,
) -> Option<(Rect, Option<Rect>)> {
    app.selected_package()?;
    let content = app.sheet.content_rect(size, theme);
    let pad = 24.0;
    let gap = 10.0;
    let height = theme.metrics.button.height.max(34.0);
    let y = content.bottom() - pad - height;
    let (primary_label, secondary_label) = detail_labels(app);
    let primary_width = Button::new(&primary_label).width(text, theme).max(96.0);
    let primary = Rect::from_xywh(
        content.right() - pad - primary_width,
        y,
        primary_width,
        height,
    );
    let secondary = secondary_label.map(|label| {
        let width = Button::new(&label).width(text, theme).max(96.0);
        Rect::from_xywh(primary.x() - gap - width, y, width, height)
    });
    Some((primary, secondary))
}

pub fn handle_events(
    app: &mut App,
    events: &[Event],
    size: Size,
    theme: &Theme,
    text: &TextEngine,
) {
    let lay = layout(size, theme, app.sidebar_visible);
    app.scroll
        .set_content_height(items_content_height(app, app.view.len(), &lay));
    app.scroll.set_offset(app.scroll.offset(), lay.viewport);

    for event in events {
        match event {
            Event::PointerMotion { position, .. } => {
                app.hover_control = HeaderBar::new()
                    .controls(ControlsSide::Right)
                    .control_at(lay.header, theme, *position);
                app.nav.hover_at(lay.nav, Some(*position), theme);
                app.hover_refresh = lay.refresh.contains(*position);
                app.hover_sidebar = lay.sidebar_toggle.contains(*position);
                app.hover_back = lay.back.contains(*position);
                app.hover_forward = lay.forward.contains(*position);
                app.hover_view = lay.view.contains(*position);
                app.hover_menu = lay.menu.contains(*position);
                app.hover_update_all = app.page == Page::Updates
                    && app.update_count() > 0
                    && lay.update_all.contains(*position);
                app.search
                    .set_hover_clear(app.search.clear_at(lay.search, theme, *position));
                if let Some(panel) = menu_panel(app, &lay, size, theme, text) {
                    app.menu.hover_at(panel, Some(*position), theme);
                }
                let hover = if app.menu_open {
                    None
                } else {
                    item_at(app, &lay, *position)
                };
                let hover_action = hover
                    .and_then(|index| {
                        let package = app.view.get(index).map(|i| &app.packages[*i])?;
                        let label = card_label(package);
                        let card = item_rect(app, &lay, index, app.scroll.offset());
                        Some(card_action_rect(card, theme, text, &label).contains(*position))
                    })
                    .unwrap_or(false);
                app.hover_card = hover;
                app.hover_action = hover_action;
            }
            Event::PointerLeave => {
                app.hover_control = None;
                app.hover_card = None;
                app.hover_action = false;
                app.hover_refresh = false;
                app.hover_sidebar = false;
                app.hover_back = false;
                app.hover_forward = false;
                app.hover_view = false;
                app.hover_menu = false;
                app.hover_update_all = false;
                app.nav.hover_at(lay.nav, None, theme);
                app.menu.hover_at(Rect::ZERO, None, theme);
                app.search.set_hover_clear(false);
            }
            Event::PointerButton(button) if button.state == ButtonState::Pressed => {
                match button.button {
                    PointerButton::Left => {
                        handle_click(app, &lay, size, theme, text, button.position)
                    }
                    PointerButton::Back => app.go_back(),
                    PointerButton::Forward => app.go_forward(),
                    _ => {}
                }
            }
            Event::Scroll(scroll)
                if lay.viewport.contains(scroll.position) && !app.sheet.is_open() =>
            {
                app.scroll
                    .on_scroll(scroll.dy / SCROLL_UNIT * SCROLL_STEP, lay.viewport);
            }
            Event::Key(key) if key.state == ButtonState::Pressed => {
                handle_key(app, key, &lay, theme);
            }
            Event::Paste(pasted) => {
                if !app.sheet.is_open() && !app.menu_open && app.search.is_focused() {
                    app.search.insert_str(pasted);
                    app.on_query_changed();
                }
            }
            Event::Text(inserted)
                if !app.sheet.is_open() && !app.menu_open && app.search.is_focused() =>
            {
                app.search.insert_str(inserted);
                app.on_query_changed();
            }
            _ => {}
        }
    }
}

fn handle_click(
    app: &mut App,
    lay: &Layout,
    size: Size,
    theme: &Theme,
    text: &TextEngine,
    point: Point,
) {
    // Panel szczegółów jest modalny.
    if app.sheet.is_open() {
        if app.sheet.close_at(size, theme, point) || app.sheet.scrim_at(size, theme, point) {
            app.close_details();
        } else if let Some((primary, secondary)) = detail_button_rects(app, size, theme, text) {
            if primary.contains(point) {
                app.detail_primary();
            } else if secondary.is_some_and(|rect| rect.contains(point)) {
                app.detail_secondary();
            }
        }
        return;
    }

    // Klik poza polem wyszukiwania zdejmuje fokus — kursor i podświetlenie znikają.
    if !lay.search.contains(point) {
        app.search.set_focused(false);
    }

    // Toast tylko w swoim prostokącie.
    if app.toast.is_open() && app.toast.rect(size, theme, text).contains(point) {
        if app.toast.action_at(size, theme, text, point) {
            // Sklep nie używa akcji w toastach — samo zamknięcie.
            app.toast.set_open_animated(false);
            app.toast_until = None;
        } else if app.toast.dismiss_at(size, theme, text, point) {
            app.toast.set_open_animated(false);
            app.toast_until = None;
        }
        return;
    }

    // Menu ☰ jest nakładką: klik wewnątrz wybiera pozycję, klik poza zamyka.
    if app.menu_open {
        if let Some(panel) = menu_panel(app, lay, size, theme, text) {
            if panel.contains(point) {
                if let Some(index) = app.menu.click_at(panel, point, theme) {
                    app.menu_action(index);
                }
            }
        }
        app.menu_open = false;
        return;
    }

    let header = HeaderBar::new().controls(ControlsSide::Right);
    if let Some(control) = header.control_at(lay.header, theme, point) {
        match control {
            WindowControl::Minimize => app.pending.minimize = true,
            WindowControl::Maximize => app.pending.maximize = true,
            WindowControl::Close => app.pending.close = true,
        }
        return;
    }

    // Kontrolki treści headerbara (jak w Luna Files).
    if lay.sidebar_toggle.contains(point) {
        app.sidebar_visible = !app.sidebar_visible;
        return;
    }
    if lay.back.contains(point) {
        app.go_back();
        return;
    }
    if lay.forward.contains(point) {
        app.go_forward();
        return;
    }
    if lay.refresh.contains(point) {
        app.refresh_all();
        return;
    }
    if lay.view.contains(point) {
        app.toggle_view_mode();
        return;
    }
    if lay.menu.contains(point) {
        app.menu_open = true;
        app.menu.set_hovered(None);
        return;
    }
    if lay.search.contains(point) {
        app.search.set_focused(true);
        if app.search.clear_at(lay.search, theme, point) {
            app.search.clear();
            app.on_query_changed();
        }
        return;
    }

    if lay.header.contains(point) {
        app.pending.begin_move = true;
        return;
    }

    if lay.nav.contains(point) {
        if let Some(index) = app.nav.index_at(lay.nav, point, theme) {
            app.nav.set_selected(Some(index));
            if let Some(page) = app.nav_pages.get(index).cloned().flatten() {
                app.go_to(page);
            }
        }
        return;
    }

    if app.page == Page::Updates && app.update_count() > 0 && lay.update_all.contains(point) {
        app.update_all();
        return;
    }

    if lay.viewport.contains(point) {
        if let Some(status) = app.status_page() {
            if status.action_at(lay.viewport, theme, point) {
                app.status_action();
            }
            return;
        }
        if let Some(view_index) = item_at(app, lay, point) {
            let Some(&package_index) = app.view.get(view_index) else {
                return;
            };
            let label = card_label(&app.packages[package_index]);
            let card = item_rect(app, lay, view_index, app.scroll.offset());
            if card_action_rect(card, theme, text, &label).contains(point) {
                app.card_action(view_index);
            } else {
                let key = app.packages[package_index].key();
                app.open_details(&key);
            }
        }
    }
}

fn handle_key(app: &mut App, key: &KeyEvent, lay: &Layout, _theme: &Theme) {
    if app.menu_open {
        if key.keysym == 0xff1b {
            app.menu_open = false;
        }
        return;
    }
    if app.sheet.is_open() {
        if key.keysym == 0xff1b {
            app.close_details();
        }
        return;
    }

    if key.modifiers.alt {
        match key.keysym {
            0xff51 => {
                app.go_back();
                return;
            }
            0xff53 => {
                app.go_forward();
                return;
            }
            _ => {}
        }
    }

    if key.modifiers.ctrl {
        match key.keysym {
            0x62 => {
                app.sidebar_visible = !app.sidebar_visible;
                return;
            }
            0x66 => {
                app.search.set_focused(true);
                app.search.select_all();
                return;
            }
            0x72 => {
                app.refresh_all();
                return;
            }
            _ => {}
        }
    }

    if app.search.is_focused() {
        if let Some(command) = clipboard_command(&mut app.search, key) {
            match command {
                TextClipboardCommand::Copy(Some(text)) | TextClipboardCommand::Cut(Some(text)) => {
                    app.clipboard_text = Some(text);
                }
                TextClipboardCommand::Copy(None) | TextClipboardCommand::Cut(None) => {}
                TextClipboardCommand::Paste => app.paste_requested = true,
            }
            app.on_query_changed();
            return;
        }
        match key.keysym {
            0xff1b => {
                if app.search.is_empty() {
                    app.search.set_focused(false);
                } else {
                    app.search.clear();
                    app.on_query_changed();
                }
                return;
            }
            0xff0d => {
                // Enter zamyka edycję wyszukiwania (wyniki zostają).
                app.search.set_focused(false);
                return;
            }
            0xff08 => {
                app.search.backspace();
                app.on_query_changed();
                return;
            }
            0xffff => {
                app.search.delete_forward();
                app.on_query_changed();
                return;
            }
            0xff51 => {
                app.search.move_left();
                return;
            }
            0xff53 => {
                app.search.move_right();
                return;
            }
            0xff50 => {
                app.search.home();
                return;
            }
            0xff57 => {
                app.search.end();
                return;
            }
            0xff54 => {
                app.scroll.on_scroll(SCROLL_STEP, lay.viewport);
                return;
            }
            0xff52 => {
                app.scroll.on_scroll(-SCROLL_STEP, lay.viewport);
                return;
            }
            0xff56 => {
                app.scroll
                    .on_scroll(lay.viewport.height() * 0.8, lay.viewport);
                return;
            }
            0xff55 => {
                app.scroll
                    .on_scroll(-lay.viewport.height() * 0.8, lay.viewport);
                return;
            }
            0x61 if key.modifiers.ctrl => {
                app.search.select_all();
                return;
            }
            _ => {}
        }
        if !key.modifiers.ctrl && !key.modifiers.alt {
            if let Some(inserted) = &key.text {
                if !inserted.is_empty() && !inserted.chars().any(char::is_control) {
                    app.search.insert_str(inserted);
                    app.on_query_changed();
                }
            }
        }
        return;
    }

    match key.keysym {
        0xff54 => {
            app.scroll.on_scroll(SCROLL_STEP, lay.viewport);
        }
        0xff52 => {
            app.scroll.on_scroll(-SCROLL_STEP, lay.viewport);
        }
        0xff56 => {
            app.scroll
                .on_scroll(lay.viewport.height() * 0.8, lay.viewport);
        }
        0xff55 => {
            app.scroll
                .on_scroll(-lay.viewport.height() * 0.8, lay.viewport);
        }
        0xff1b => {}
        _ => {
            // Pisanie poza polem wyszukiwania od razu je aktywuje.
            if !key.modifiers.ctrl && !key.modifiers.alt {
                if let Some(inserted) = &key.text {
                    if !inserted.is_empty() && !inserted.chars().any(char::is_control) {
                        app.search.set_focused(true);
                        app.search.insert_str(inserted);
                        app.on_query_changed();
                    }
                }
            }
        }
    }
}

fn elide(ctx: &PaintCtx, text: &str, max_width: f64, size: f64, weight: u16) -> String {
    if max_width <= 0.0 {
        return String::new();
    }
    let full = ctx.measure(text, size, weight);
    if full <= max_width {
        return text.to_owned();
    }
    let ellipsis = ctx.measure("…", size, weight);
    let budget = (max_width - ellipsis).max(0.0);
    if budget <= 0.0 {
        return "…".to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    let approx = ((chars.len() as f64) * budget / full).floor() as usize;
    let mut keep = approx.min(chars.len());
    loop {
        let candidate: String = chars[..keep].iter().collect();
        if ctx.measure(&candidate, size, weight) <= budget || keep == 0 {
            let mut out = candidate;
            out.push('…');
            return out;
        }
        keep -= 1;
    }
}

fn wrap(
    ctx: &PaintCtx,
    text: &str,
    max_width: f64,
    size: f64,
    weight: u16,
    max_lines: usize,
) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let candidate = if current.is_empty() {
            word.to_owned()
        } else {
            format!("{current} {word}")
        };
        if current.is_empty() || ctx.measure(&candidate, size, weight) <= max_width {
            current = if current.is_empty() {
                elide(ctx, &candidate, max_width, size, weight)
            } else {
                candidate
            };
        } else {
            lines.push(std::mem::take(&mut current));
            current = word.to_owned();
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.len() > max_lines {
        lines.truncate(max_lines);
        if let Some(last) = lines.last_mut() {
            let more = format!("{last} …");
            *last = elide(ctx, &more, max_width, size, weight);
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::config::{Commands, Enabled, Privilege, RepoConfig, StoreConfig};
    use crate::model::{BackendKind, RawPackage};
    use libluna_core::{Point, Size, Theme};
    use libluna_render::{Scene, TextEngine};

    fn theme() -> Theme {
        Theme::light()
    }

    fn test_app() -> App {
        let source = format!("ui-{}", std::process::id());
        let cfg = StoreConfig {
            refresh_minutes: 60,
            privilege: Privilege::None,
            featured: vec!["firefox".into()],
            repos: vec![RepoConfig {
                id: source.clone(),
                name: "Test".into(),
                backend: BackendKind::Pacman,
                enabled: Enabled::Yes,
                remote: None,
                installation: None,
                probe: None,
                overrides: Commands::default(),
            }],
            path: None,
            warning: None,
        };
        let mut app = App::from_config(cfg);
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
        app.installed.insert(source.clone(), Default::default());
        app.updates.insert(source.clone(), Default::default());
        app.rebuild_source_packages(&source);
        app.data_version += 1;
        app.dirty = true;
        app.rebuild_nav();
        app.sync_view();
        app
    }

    #[test]
    fn paint_handles_empty_and_populated_states() {
        let app = test_app();
        let theme = theme();
        let text = TextEngine::none();
        let mut scene = Scene::new();
        paint(&app, &mut scene, Size::new(1120.0, 760.0), &theme, &text);

        let mut empty = App::from_config(StoreConfig {
            refresh_minutes: 60,
            privilege: Privilege::None,
            featured: Vec::new(),
            repos: Vec::new(),
            path: None,
            warning: None,
        });
        empty.sync_view();
        paint(&empty, &mut scene, Size::new(800.0, 600.0), &theme, &text);
    }

    #[test]
    fn clicking_a_card_opens_details() {
        let mut app = test_app();
        let theme = theme();
        let text = TextEngine::none();
        let size = Size::new(1120.0, 760.0);
        let lay = layout(size, &theme, true);
        let card = item_rect(&app, &lay, 0, 0.0);
        // Klik w lewą część karty (nie w przycisk akcji).
        let point = Point::new(card.x() + 20.0, card.y() + 20.0);
        handle_click(&mut app, &lay, size, &theme, &text, point);
        assert!(app.sheet.is_open());
        assert_eq!(
            app.selected_package().map(|package| package.id.as_str()),
            Some("firefox")
        );
    }

    #[test]
    fn clicking_a_card_shows_the_sheet_after_animation() {
        use libluna_core::{ButtonState, Event, PointerButton, PointerEvent};
        let mut app = test_app();
        let theme = theme();
        let text = TextEngine::none();
        let size = Size::new(1120.0, 760.0);
        let lay = layout(size, &theme, app.sidebar_visible);
        let card = item_rect(&app, &lay, 0, 0.0);
        let point = Point::new(card.x() + 20.0, card.y() + 20.0);
        handle_events(
            &mut app,
            &[Event::PointerButton(PointerEvent {
                position: point,
                button: PointerButton::Left,
                state: ButtonState::Pressed,
                modifiers: Default::default(),
            })],
            size,
            &theme,
            &text,
        );
        assert!(app.sheet.is_open());
        for _ in 0..10 {
            app.tick(0.05);
        }
        assert!(
            app.sheet.progress() > 0.99,
            "panel szczegółów musi być w pełni wysunięty"
        );
    }

    #[test]
    fn elide_and_wrap_are_safe_without_fonts() {
        let text = TextEngine::none();
        let theme = theme();
        let mut scene = Scene::new();
        let ctx = PaintCtx::new(&mut scene, &theme, &text);
        assert_eq!(elide(&ctx, "hello", 100.0, 14.0, weight::NORMAL), "hello");
        let lines = wrap(&ctx, "one two three", 100.0, 14.0, weight::NORMAL, 2);
        assert!(!lines.is_empty());
    }

    #[test]
    fn header_has_search_and_toggles_sidebar_and_menu() {
        let mut app = test_app();
        let theme = theme();
        let text = TextEngine::none();
        let size = Size::new(1120.0, 760.0);
        let lay = layout(size, &theme, app.sidebar_visible);
        assert!(lay.search.bottom() <= lay.header.bottom());
        assert!(lay.search.width() > 200.0);

        handle_click(
            &mut app,
            &lay,
            size,
            &theme,
            &text,
            lay.sidebar_toggle.center(),
        );
        assert!(!app.sidebar_visible);
        let hidden = layout(size, &theme, app.sidebar_visible);
        assert_eq!(hidden.sidebar.width(), 0.0);
        assert_eq!(hidden.content.x(), 0.0);

        let lay = layout(size, &theme, app.sidebar_visible);
        handle_click(&mut app, &lay, size, &theme, &text, lay.menu.center());
        assert!(app.menu_open);
        // Rysowanie menu nie może się wywrócić.
        let mut scene = Scene::new();
        paint(&app, &mut scene, size, &theme, &text);
        // Klik poza menu zamyka je i nie klika karty pod spodem.
        handle_click(
            &mut app,
            &lay,
            size,
            &theme,
            &text,
            Point::new(size.width - 3.0, size.height - 3.0),
        );
        assert!(!app.menu_open);
    }

    #[test]
    fn header_view_toggle_and_back_button_work() {
        let mut app = test_app();
        let theme = theme();
        let text = TextEngine::none();
        let size = Size::new(1120.0, 760.0);
        let lay = layout(size, &theme, app.sidebar_visible);

        handle_click(&mut app, &lay, size, &theme, &text, lay.view.center());
        assert_eq!(app.view_mode, ViewMode::List);
        let mut scene = Scene::new();
        paint(&app, &mut scene, size, &theme, &text);
        handle_click(&mut app, &lay, size, &theme, &text, lay.view.center());
        assert_eq!(app.view_mode, ViewMode::Grid);

        app.go_to(Page::All);
        app.go_to(Page::Installed);
        let lay = layout(size, &theme, app.sidebar_visible);
        handle_click(&mut app, &lay, size, &theme, &text, lay.back.center());
        assert_eq!(app.page, Page::All);
        handle_click(&mut app, &lay, size, &theme, &text, lay.forward.center());
        assert_eq!(app.page, Page::Installed);
    }

    #[test]
    fn clicking_header_search_focuses_it() {
        let mut app = test_app();
        let theme = theme();
        let text = TextEngine::none();
        let size = Size::new(1120.0, 760.0);
        let lay = layout(size, &theme, app.sidebar_visible);
        handle_click(&mut app, &lay, size, &theme, &text, lay.search.center());
        assert!(app.search.is_focused());
    }

    #[test]
    fn clicking_elsewhere_hides_search_focus() {
        let mut app = test_app();
        let theme = theme();
        let text = TextEngine::none();
        let size = Size::new(1120.0, 760.0);
        let lay = layout(size, &theme, app.sidebar_visible);
        app.search.set_focused(true);
        // Klik w pasek boczny (poza polem) zamyka edycję wyszukiwania.
        let nav = lay.nav;
        let point = Point::new(nav.x() + 10.0, nav.y() + 40.0);
        handle_click(&mut app, &lay, size, &theme, &text, point);
        assert!(!app.search.is_focused());
    }

    #[test]
    fn search_result_page_has_its_own_title() {
        let mut app = test_app();
        app.search.set_text("fire");
        app.on_query_changed();
        assert_eq!(page_title(&app), tr("Search results"));
        assert_eq!(app.view.len(), 1);
    }

    #[test]
    fn search_finds_packages_outside_the_current_page() {
        let mut app = test_app();
        // Na „Polecane” jest tylko Firefox; GIMP musi się znaleźć globalnie.
        assert_eq!(app.view.len(), 1);
        app.search.set_text("gimp");
        app.on_query_changed();
        assert_eq!(app.view.len(), 1);
        assert_eq!(app.packages[app.view[0]].id, "gimp");
    }

    #[test]
    fn details_sheet_is_painted_after_animation() {
        let mut app = test_app();
        let key = app.packages[0].key();
        app.open_details(&key);
        for _ in 0..10 {
            app.tick(0.05);
        }
        assert!(app.sheet.progress() > 0.99);
        let theme = theme();
        let text = TextEngine::none();
        let size = Size::new(1120.0, 760.0);
        let mut scene = Scene::new();
        paint(&app, &mut scene, size, &theme, &text);
        let mut renderer = libluna_render::SoftwareRenderer::new(1120, 760);
        renderer.draw(&scene, 1.0, theme.colors.window_bg);
        let px = |point: Point| {
            let x = point.x as usize;
            let y = point.y as usize;
            let i = (y * 1120 + x) * 4;
            renderer.data()[i..i + 4].to_vec()
        };
        let panel = app.sheet.content_rect(size, &theme);
        assert_ne!(
            px(panel.center()),
            px(Point::new(5.0, 5.0)),
            "panel szczegółów musi być widoczny po animacji"
        );
    }

    #[test]
    fn typing_in_search_filters_results() {
        use libluna_core::{ButtonState, KeyEvent};
        let mut app = test_app();
        let theme = theme();
        let text = TextEngine::none();
        let size = Size::new(1120.0, 760.0);
        let lay = layout(size, &theme, app.sidebar_visible);
        handle_click(&mut app, &lay, size, &theme, &text, lay.search.center());
        assert!(app.search.is_focused());
        handle_key(
            &mut app,
            &KeyEvent {
                keysym: 0x67,
                keycode: 0,
                text: Some("gimp".to_owned()),
                state: ButtonState::Pressed,
                modifiers: Default::default(),
                repeat: false,
            },
            &lay,
            &theme,
        );
        assert_eq!(app.search.text(), "gimp");
        assert_eq!(app.view.len(), 1);
        assert_eq!(app.packages[app.view[0]].id, "gimp");
    }

    #[test]
    fn layout_is_stable_on_small_windows() {
        let theme = theme();
        let lay = layout(Size::new(420.0, 320.0), &theme, true);
        assert!(lay.viewport.width() > 0.0);
        assert!(lay.viewport.height() >= 0.0);
        assert!(grid_cols(lay.viewport.width()) >= 1);
    }
}

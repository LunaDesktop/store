//! Powiadomienia systemowe Luna Store.
//!
//! Korzystamy bezpośrednio z `org.freedesktop.Notifications` (serwerem jest
//! `luna-shell`), a nie z `notify-send`. Hint `desktop-entry` mówi powłoce,
//! którą aplikację otworzyć po kliknięciu powiadomienia — jeśli okno Sklepu
//! już istnieje, `luna-shell` je aktywuje zamiast uruchamiać drugie.
//!
//! Wysyłka idzie w wątku: D-Bus może chwilowo nie odpowiadać (np. powłoka
//! startuje), a UI nie może się przez to zacinać.

use std::collections::HashMap;

use libluna_i18n::{tr, tr_args};
use zbus::zvariant::Value;

/// Nazwa wyświetlana nadawcy w toaście.
const APP_NAME: &str = "Luna Store";
/// Wpis `.desktop` — po kliknięciu powiadomienia powłoka otworzy ten Sklep.
const DESKTOP_ENTRY: &str = "eu.luna.Store.desktop";
/// Ikona z motywu systemowego (taka jak w pliku `.desktop`).
const ICON: &str = "system-software-install";
/// Maksymalna długość treści — toasty nie są miejscem na cały log menedżera.
const BODY_LIMIT: usize = 240;

/// Wysyła powiadomienie bez blokowania UI.
fn send(summary: String, body: String, timeout_ms: i32, critical: bool) {
    std::thread::spawn(move || {
        if let Err(err) = send_blocking(&summary, &body, timeout_ms, critical) {
            eprintln!("luna-store: nie udało się wysłać powiadomienia: {err}");
        }
    });
}

fn send_blocking(summary: &str, body: &str, timeout_ms: i32, critical: bool) -> Result<(), String> {
    let connection =
        zbus::blocking::Connection::session().map_err(|err| format!("D-Bus: {err}"))?;
    // `default` sprawia, że kliknięcie treści (nie tylko przycisku) wywołuje akcję.
    let open_label = tr("Open Store");
    let actions: Vec<&str> = vec!["default", open_label.as_str()];
    let urgency: u8 = if critical { 2 } else { 1 };
    let mut hints: HashMap<&str, Value<'_>> = HashMap::new();
    hints.insert("desktop-entry", Value::from(DESKTOP_ENTRY));
    hints.insert("urgency", Value::from(urgency));
    hints.insert("image-path", Value::from(ICON));
    connection
        .call_method(
            Some("org.freedesktop.Notifications"),
            "/org/freedesktop/Notifications",
            Some("org.freedesktop.Notifications"),
            "Notify",
            &(
                APP_NAME, 0u32, ICON, summary, body, actions, hints, timeout_ms,
            ),
        )
        .map_err(|err| format!("D-Bus: {err}"))?;
    Ok(())
}

/// Skraca treść do jednej linii o rozsądnej długości.
fn shorten(text: &str) -> String {
    let mut out = String::new();
    let mut last_was_space = false;
    for ch in text.chars() {
        let ch = if ch.is_control() { ' ' } else { ch };
        if ch == ' ' {
            if last_was_space || out.is_empty() {
                continue;
            }
            last_was_space = true;
        } else {
            last_was_space = false;
        }
        out.push(ch);
        if out.chars().count() >= BODY_LIMIT {
            out.push('…');
            break;
        }
    }
    out.trim_end().to_owned()
}

/// Instalacja zakończona sukcesem.
pub fn installed(package: &str) {
    send(tr("Installed"), shorten(package), 6000, false);
}

/// Usunięcie pakietu.
pub fn removed(package: &str) {
    send(tr("Removed"), shorten(package), 6000, false);
}

/// Aktualizacja jednego pakietu.
pub fn updated(package: &str) {
    send(tr("Updated"), shorten(package), 6000, false);
}

/// Zakończone „Aktualizuj wszystko”.
pub fn updates_installed(names: &str) {
    send(tr("All updates installed"), shorten(names), 8000, false);
}

/// Operacja nie powiodła się — toast zostaje, dopóki użytkownik go nie zamknie.
pub fn failed(detail: &str) {
    let body = shorten(detail);
    send(
        tr("Failed"),
        if body.is_empty() {
            tr("Operation failed")
        } else {
            body
        },
        0,
        true,
    );
}

/// System wykrył nowe aktualizacje (raz na wzrost ich liczby — patrz `App`).
pub fn updates_available(count: usize) {
    let body = tr_args(
        "{n} updates are ready to install.",
        &[("n", &count.to_string())],
    );
    send(tr("Updates available"), body, 8000, false);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shorten_collapses_whitespace_and_control_chars() {
        assert_eq!(shorten("  a\n\tb   c  "), "a b c");
    }

    #[test]
    fn shorten_limits_long_output() {
        let long = "x".repeat(BODY_LIMIT * 2);
        let out = shorten(&long);
        assert_eq!(out.chars().count(), BODY_LIMIT + 1);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn shorten_keeps_utf8_boundaries() {
        let text = "ą".repeat(BODY_LIMIT + 10);
        let out = shorten(&text);
        assert_eq!(out.chars().count(), BODY_LIMIT + 1);
    }

    #[test]
    fn notify_arguments_match_the_specification() {
        // `actions` musi być tablicą napisów (`as`), a nie krotką — inaczej
        // serwer odpowiada „Signature mismatch”.
        let open = "Open Store";
        let actions: Vec<&str> = vec!["default", open];
        let hints: HashMap<&str, Value<'_>> = HashMap::new();
        let body = (
            "Luna Store",
            0u32,
            ICON,
            "title",
            "body",
            actions,
            hints,
            5000i32,
        );
        use zbus::zvariant::serialized::Context;
        use zbus::zvariant::{DynamicType, Endian};
        let data = zbus::zvariant::to_bytes(Context::new_dbus(Endian::Little, 0), &body)
            .expect("serializacja");
        assert!(!data.bytes().is_empty());
        assert_eq!(DynamicType::signature(&body).to_string(), "(susssasa{sv}i)");
    }

    /// Wymaga sesyjnej magistrali D-Bus i działającej powłoki — pomijany w CI.
    #[test]
    #[ignore = "wymaga sesyjnej magistrali D-Bus"]
    fn send_reaches_the_notification_server() {
        match send_blocking("Luna Store", "test", 1000, false) {
            Ok(()) => {}
            Err(err) => assert!(err.contains("D-Bus"), "oczekiwano błędu D-Bus, jest: {err}"),
        }
    }
}

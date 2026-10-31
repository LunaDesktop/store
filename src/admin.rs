//! Autoryzacja administracyjna: kiedy polkit działa, a kiedy trzeba hasła w aplikacji.
//!
//! Na większości systemów `pkexec` jest **setuid root** i pokazuje okno polkit
//! (`luna-polkit`). Zdarzają się jednak obrazy (np. PaganOS 0.5.3), w których
//! `pkexec` nie ma bitu setuid — wtedy każde polecenie kończy się komunikatem
//! „pkexec must be setuid root” i kodem 127. W takim wypadku Sklep prosi
//! o hasło we własnym oknie i przekazuje je do `sudo -S` przez `stdin`.

use std::path::Path;

use crate::config::{which, Privilege};

/// Czy `pkexec` istnieje i jest setuid root (czyli realnie podniesie uprawnienia).
pub fn pkexec_usable() -> bool {
    which("pkexec").is_some_and(|path| is_setuid_root(&path))
}

/// Czy `sudo` istnieje i jest setuid root.
pub fn sudo_usable() -> bool {
    which("sudo").is_some_and(|path| is_setuid_root(&path))
}

/// Czy operacja będzie wymagać **okna hasła w aplikacji** (bo nie ma sprawnego pkexec).
pub fn needs_password_prompt(privilege: Privilege) -> bool {
    match privilege {
        Privilege::None => false,
        // Jawnie wybrany pkexec: nie podmieniamy go po cichu — błąd trafi do logu.
        Privilege::Pkexec => false,
        Privilege::Sudo => sudo_usable(),
        Privilege::Auto => !pkexec_usable() && sudo_usable(),
    }
}

/// Czy w ogóle da się podnieść uprawnienia przy tej konfiguracji.
pub fn available(privilege: Privilege) -> bool {
    match privilege {
        Privilege::None => true,
        Privilege::Pkexec => pkexec_usable(),
        Privilege::Sudo => sudo_usable(),
        Privilege::Auto => pkexec_usable() || sudo_usable(),
    }
}

#[cfg(unix)]
fn is_setuid_root(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path)
        .map(|meta| meta.mode() & 0o4000 != 0 && meta.uid() == 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_setuid_root(_path: &Path) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_is_needed_only_when_pkexec_is_broken() {
        if pkexec_usable() {
            assert!(!needs_password_prompt(Privilege::Auto));
        } else if sudo_usable() {
            assert!(needs_password_prompt(Privilege::Auto));
            assert!(needs_password_prompt(Privilege::Sudo));
        }
        assert!(!needs_password_prompt(Privilege::None));
    }

    #[test]
    fn availability_matches_configuration() {
        assert!(available(Privilege::None));
        if pkexec_usable() {
            assert!(available(Privilege::Pkexec));
        }
        if sudo_usable() {
            assert!(available(Privilege::Sudo));
        }
    }
}

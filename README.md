# Luna Store

Sklep z oprogramowaniem Luny na libLunie: natywne menedżery pakietów
(pacman, pag, apt, dnf, emerge), Flatpak i aktualizacje.

```bash
cargo run
cargo test

# podgląd do PNG bez Waylanda
cargo run --bin store-render -- --dark /tmp/luna-store.png
```

Źródła opisuje `data/repositories.conf` (kopiowany przez instalator do
`/usr/share/luna/store/`, nadpisywany przez `~/.config/luna/store/repositories.conf`).
Pełna dokumentacja: [`docs/store.md`](../../docs/store.md).

Najważniejsze cechy:

- lista repozytoriów z pliku, jeden plik na wszystkie dystrybucje,
- cache katalogów i praca w tle (UI nie czeka na menedżery),
- headerbar w stylu Luna Files: sidebar (`Ctrl+B`), Wstecz/Dalej
  (`Alt+←`/`Alt+→`), wyszukiwarka, odświeżanie, **siatka/lista** i menu ☰,
- **globalne wyszukiwanie** po wszystkich źródłach (ID, nazwa, opis) z własną
  stroną wyników — działa też z „Polecanych”, gdzie wcześniej filtr obejmował
  tylko polecane aplikacje; skrót `/` lub `Ctrl+F`,
- kliknięcie karty otwiera **panel szczegółów** (BottomSheet z animacją i wyraźnym
  tłem karty), a nie od razu instaluje; przycisk na karcie nadal instaluje jednym kliknięciem,
- **zaznaczanie wielu pakietów** (przycisk ✓ w headerbarze, `Ctrl+A`, `Esc`)
  i „Zainstaluj wszystkie (N)” w pasku na dole — seria idzie po kolei, jednym
  hasłem, a po zakończeniu lista jest odblokowana;
- **Aktualizuj wszystko** pyta o potwierdzenie i wymaga hasła (`pkexec` → polkit),
  a aktualizacja leci w tle; strona **Status** pokazuje bieżącą operację, stan
  źródeł, historię i dziennik `~/.cache/luna/store/operations.log`,
- gdy `pkexec` nie jest setuid (kod 127), Sklep prosi o hasło we własnym oknie
  i używa `sudo -S` — hasło idzie przez `stdin`, nigdy przez `argv` ani log,
- **cykliczny sync w tle**: co `refresh_minutes` minut (domyślnie 60) Sklep
  sprawdza statusy aktualizacji wszystkich źródeł, a katalogi odświeża tylko
  gdy cache jest starszy; nowe aktualizacje wywołują powiadomienie, a ręczne
  odświeżenie resetuje licznik,
- **powiadomienia systemowe** po instalacji/aktualizacji, o dostępnych
  aktualizacjach i o błędach; kliknięcie otwiera Sklep albo aktywuje jego
  istniejące okno (`desktop-entry = eu.luna.Store.desktop`),
- „O aplikacji” i **„Konfiguracja repozytorium”** otwierają modalne okna (ścieżka,
  lista źródeł, przycisk otwarcia pliku przez `xdg-open`).
- filtrowanie w pamięci i wirtualizowana siatka/lista kart,
- instalacja/usuwanie/aktualizacja przez **`pkexec`** (hasło pyta agent polkit
  Luny, `luna-polkit`) albo Flatpaka; brak sprawnego narzędzia autoryzacji to
  czytelny błąd, nie cicha próba uruchomienia menedżera bez uprawnień,
- `backend = custom` dla dowolnego menedżera pakietów.

## Licencja

GNU GPL v3.0 lub nowsza (`GPL-3.0-or-later`) — pełny tekst w [`LICENSE`](LICENSE).
© 2026 Luna Project.

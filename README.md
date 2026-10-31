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
  tylko polecane aplikacje,
- kliknięcie karty otwiera **panel szczegółów** (BottomSheet z animacją), a nie
  od razu instaluje; przycisk na karcie nadal instaluje jednym kliknięciem,
- filtrowanie w pamięci i wirtualizowana siatka/lista kart,
- instalacja/usuwanie/aktualizacja przez **`pkexec`** (hasło pyta agent polkit
  Luny, `luna-polkit`) albo Flatpaka; brak `pkexec` to czytelny błąd, nie cicha
  próba uruchomienia menedżera bez uprawnień,
- `backend = custom` dla dowolnego menedżera pakietów.

## Licencja

GNU GPL v3.0 lub nowsza (`GPL-3.0-or-later`) — pełny tekst w [`LICENSE`](LICENSE).
© 2026 Luna Project.

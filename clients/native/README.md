# proteus-native

Экспериментальный нативный клиент на GPUI (`gpui-kit`). Работает с тем же
HTTP/SSE app-server, что и `clients/app`, через DTO из
`proteus-contracts::app_protocol`; основной клиент остаётся в `clients/app`.

Пакет исключён из root workspace и собирается на Rust 1.95
(`rust-toolchain.toml`):

```bash
cd clients/native
cargo build --release
./target/release/proteus-native [workspace] [--config <profile|path>]
./target/release/proteus-native [workspace] --origin <url> --token <token>
```

Без `--origin` клиент сам запускает `proteus server http` для workspace
(по умолчанию — текущий каталог). Бинарь сервера берётся из `PROTEUS_BIN`,
затем рядом с клиентом, затем из `PATH`; профиль по умолчанию — из
настроек desktop-приложения. Сервер получает `PR_SET_PDEATHSIG` и
завершается вместе с клиентом.

Устройство:

- `transcript` — headless модель ленты: snapshot и live fold runtime
  событий по правилам `app_server/turn_progress.rs`, без GPUI;
- `tool_display` — заголовки инструментов, разбор patch и плана;
- `settings` — headless черновик и страницы настроек агента поверх
  `/config/builder`: поля строятся по `config_schema` модулей и
  раскладываются по разделам «Модель», «Доступ», «Контекст»,
  «Инструменты», «Сценарий», «Хуки» (неизвестные слоты получают свою
  страницу); сохранение отправляет только изменённые разделы;
- `api`, `connection` — блокирующие HTTP-запросы и SSE-поток в отдельных
  потоках;
- `ui` — окно: список сессий, лента, подтверждения, поле ввода, экран
  настроек и правая панель сведений (контекст, план, лимиты модели из
  `/model/quota`, сессия, агент, заметки в
  `~/.local/share/dev.proteus.native/notes.md`); `ui/motion.rs` — общие
  анимации переходов и раскрытия.

Клавиши: `Ctrl+,` — настройки, `Ctrl+Alt+B` — панель сведений,
`Ctrl+N` — новая сессия.

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
- `settings` — headless черновик и каталог страниц настроек агента поверх
  `/config/builder` и `/config/history`: страница на каждый слот (Model,
  Workflow, Context, Context Providers, Compactor, Tools, Tool Exposure,
  Policy, Hooks; неизвестные слоты добавляются по порядку), Plugins и
  History с откатом; поля строятся по `config_schema` модулей, сохранение
  отправляет только изменённые разделы;
- `preferences` — настройки самого клиента (тема, анимации, отправка
  сообщения, включённые расширения и порядок виджетов) в
  `~/.config/dev.proteus.native/preferences.json`;
- `files` — headless DTO `/workspace/*` и модель дерева: ленивые листинги,
  отметки git, фильтр по открытым папкам;
- `api`, `connection` — блокирующие HTTP-запросы и SSE-поток в отдельных
  потоках;
- `ui` — окно: список сессий, лента, подтверждения, поле ввода, экран
  настроек (блоки «Агент» и «Клиент») и правая панель; `ui/motion.rs` —
  общие анимации переходов и раскрытия с учётом выбранного уровня анимаций.

Расширения клиента (`ui/extensions`) описываются `Manifest` и получают
доступ к окну только через `Host` (чтение сессии, API, события
`SessionOpened`/`RunSettled`/`PanelOpened`, открытие страниц). Встроенные:
контекст, план, файлы (виджет «Изменения» и страница «Файлы» с деревом,
просмотром и diff против HEAD), лимиты модели (`/model/quota`), сессия,
агент, заметки (`~/.local/share/dev.proteus.native/notes.md`). Расширения
включаются в настройках «Клиент → Расширения»; виджеты панели
переставляются перетаскиванием заголовка или кнопками там же.

Клавиши: `Ctrl+,` — настройки, `Ctrl+Alt+B` — правая панель,
`Ctrl+N` — новая сессия, `Ctrl+Tab`/`Ctrl+Shift+Tab` — чат и страницы
расширений, `Alt+↑`/`Alt+↓` — соседняя страница настроек. В списке файлов:
стрелки, `Home`/`End`, `Enter`/`Space` — свернуть папку, `D` — файл или
изменения, `/` или `Ctrl+F` — фильтр.

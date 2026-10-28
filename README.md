# Proteus Harness

<img src="https://raw.githubusercontent.com/Proteus-harness/Proteus-desktop/main/assets/proteus-logo.svg" width="112" height="112" alt="Логотип Proteus">

Proteus — конструктор ИИ-агентов на Rust. Агент собирается из модулей,
подключённых к слотам: можно выбрать алгоритм выполнения задач, модель,
поиск, память, правила работы с инструментами и обработку истории.

```text
Агент → слоты → модули
```

Слот задаёт контракт поведения. Модуль реализует этот контракт и запускается
в отдельном процессе; один модуль может обслуживать несколько слотов.
Core отвечает за сборку, выполнение запросов, разрешения и сохранение истории.
Модули можно писать на любом языке, поддерживающем общий протокол обмена.

Для Rust доступен experimental
[SDK процессных модулей](docs/guides/rust-module-sdk.md) и
[самостоятельный tool starter](examples/modules/rust-tool/README.md).
SDK не требует зависимости от Core runtime или reference implementations.

Поставляются готовые профили и примеры модулей. Один из рабочих примеров —
сборка с циклом выполнения задач, близким к Codex. Собственные алгоритмы
и сочетания модулей — основное назначение Proteus.

Развитие идёт через существующие слоты и замену их реализаций. Workflow уже
может работать без чата и модели. Публичного входа произвольной предметной
задачи с сохранением её исхода пока нет. Более общее устройство вводится под
конкретную потребность; задел и ограничения описаны в
[execution-runtime.md](docs/architecture/execution-runtime.md).

Текущая поставка — `0.1.0-alpha.3`, experimental. Версия фиксирует исходники
и состав поставки; форматы конфигурации, API, SDK и истории могут меняться
между alpha-выпусками. Подробное состояние и ограничения
собраны в [scope.md](docs/product/scope.md).

## Возможности

- **Изображения во входе:** текст и фотографии в одном сообщении, миниатюры
  в приложении и сохранение вложений вместе с сессией. Модель должна явно
  поддерживать изображения; см. [вход с изображениями](docs/guides/images.md).
- **Сборка агента из модулей:** выполнение задач, модель, поиск, память,
  контекст, правила разрешений, изменение файлов и сжатие истории.
- **Работа с моделями:** OpenAI, совместимые API, Anthropic и подключение
  подписки ChatGPT через OAuth; для локальных примеров есть `fake`.
- **Инструменты и обработчики:** единые проверки разрешений, выбор доступных
  инструментов и упорядоченные обработчики слота `hook`.
- **История работы:** сохранение сессий, продолжение задач, журнал событий
  и воспроизведение записанных запусков в поддерживаемых сценариях.
- **Сотрудничество агентов:** делегирование задач другим локальным экземплярам
  Proteus, сообщения и адресная отмена.
- **Основное приложение:** чат, проекты, настройки и диагностика в одном окне.
- **Расширения клиента:** значки, панели, вкладки документов и страницы
  настроек, в том числе внешние программы без пересборки приложения.
- **CLI и редакторы:** выполнение задач из терминала и подключение
  по Agent Client Protocol (ACP).
- **Задачи без модели:** отдельные алгоритмы, например проверка проекта,
  могут выполняться без обращения к языковой модели.

## Репозитории И Запуск

| Репозиторий | Ответственность |
|---|---|
| **Proteus-core** (этот) | Core, публичные контракты, process host, HTTP/SSE и CLI |
| [Proteus-modules](https://github.com/Proteus-harness/Proteus-modules) | Реализации по слотам и reference executable |
| [Proteus-desktop](https://github.com/Proteus-harness/Proteus-desktop) | Приложение GPUI и общие правила клиента |
| [Proteus-desktop-extensions](https://github.com/Proteus-harness/Proteus-desktop-extensions) | Поставляемые по умолчанию процессные расширения приложения |

Для CLI нужны Rust, Git и Python 3:

```bash
git clone https://github.com/Proteus-harness/Proteus-core.git
cd Proteus-core
./install.sh
```

Установщик собирает harness и отдельный `proteus-reference-module` из
Proteus-modules, затем атомарно публикует их в `~/.proteus/current` и команды
в `~/.local/bin`. Исходники модулей выбираются через `repositories.json`:
сначала явный `PROTEUS_MODULES_SOURCE`, затем соседний checkout
`../Proteus-modules` или загрузка зафиксированной ревизии в
`target/sources/modules`. `CARGO_TARGET_DIR`
поддерживается. Для одной сборки harness достаточно `cargo build --release -p proteus-core`.

```bash
proteus-reference-module auth openai_codex login
cd /path/to/project
proteus --config codex "Расскажи о структуре этого проекта"
```

Профили и prompts из `configs/` устанавливаются вместе с CLI. Пользовательские
настройки и авторизация остаются в прежних местах; имя Git-репозитория их не
меняет. `proteus` без запроса открывает интерактивный CLI.

Приложение собирается в репозитории
[Proteus-desktop](https://github.com/Proteus-harness/Proteus-desktop#portable):
`python3 scripts/portable.py --fast`. Готовая папка включает harness, модуль
и расширения и работает без исходных checkout.

Для Linux есть отдельный рабочий профиль `restricted` с частичной изоляцией:
`proteus --config restricted` или `./proteus-native /path/to/project --config restricted`
из portable. Он сохраняет подписку, tools и роли; ограничения применяются
к шести выбранным процессам, а не ко всему агенту. Нужны рабочий `/usr/bin/bwrap`,
user namespaces и явные runtime-пути. Покрытие и подготовка описаны в
[руководстве конфигурации](docs/guides/configuration.md#рабочий-linux-профиль-restricted).
Обычный запуск и профиль `codex` сохраняют прежнее поведение.
Для stdio MCP ограничения включаются отдельно в `tools.mcp_servers[].sandbox`:
весь сервер получает явные filesystem/network grants, независимо от количества
его tools. Пример — в [разделе MCP](docs/guides/configuration.md#mcp).

Авторизация и собственные профили — в
[руководстве конфигурации](docs/guides/configuration.md).

## Подключение Своего Модуля

Конфигурация выбирает реализацию слота и описывает запуск предоставляющего
её модуля. Например, фрагмент подключения поиска на Python:

```toml
[modules]
context = "simple"

[tools]
enabled = ["search"]

[components.python-search]
command = "python3"
args = ["/path/to/search.py"]

[components.python-search.exports.tool.python_rg]
timeout_ms = 60000

[components.python-search.exports.context_provider.python_rg]

[components.reference-context]
command = "proteus-reference-module"

[components.reference-context.exports.context.simple]

[module_config.context.simple]
search_provider = "python_rg"
```

`components` содержит команды запуска, `exports` — доступные реализации
слотов. `modules` выбирает single-selection реализацию, `tools.enabled` — tools,
а настройки context — providers для автоматического чтения. Отдельных слотов
поиска, памяти и patch нет: явные действия — tools, автоматическое чтение —
context providers. Внутренние настройки
алгоритма задаются в `module_config.<slot>.<module_id>`.

Один процесс может предоставлять несколько слотов. Их вызовы управляются
Core, а разрешённые операции определяются контрактом активного слота
и условиями вызова.

Готовый [пример поиска на Python](examples/modules/search-process/README.md)
и [полный профиль](examples/configs/proteus.process-search.example.toml)
показывают подключение внешней программы. [Поиск на tgrep](examples/modules/tgrep-search/README.md)
заменяет только tool `search`, сохраняя существующие контракты.
Контракты и порядок добавления
модуля описаны в [modules.md](docs/architecture/modules.md).

## Полезные Команды

После установки:

В CLI REPL и приложении `/help` показывает общий каталог slash-команд сборки:
служебные `/status`, `/history`, `/clear`, `/usage`, `/model`, `/mode`,
module-owned команды и prompt-команды профиля. В приложении `/` открывает
подсказки; `//текст` отправляет буквальный `/текст` агенту.
[Семантика и настройка](docs/guides/runtime-and-events.md#slash-команды).

```bash
# Проверить настройки и команды запуска
proteus --config codex doctor

# Посмотреть доступные модули и инструменты
proteus --config codex modules list
proteus --config codex tools list

# Посмотреть выбранную сборку до запуска модулей
proteus --config codex inspect plan

# Посмотреть схему подключений
proteus --config codex inspect topology --format map

# Подключить агент к редактору по ACP
proteus --config codex server acp
```

Для работы непосредственно из исходников можно собрать сервер и модуль:

```bash
cargo build -p proteus-core
python3 scripts/build_module.py --target-dir target
PATH="$PWD/target/debug:$PATH" ./target/debug/proteus --config examples/configs/proteus.example.toml "Покажи пример ответа"
```

Этот пример использует `fake` и не требует доступа к внешней модели.

## Структура Репозитория

| Каталог | Содержимое |
|---|---|
| `crates/` | Core, контракты модулей, протокол обмена и управление процессами |
| `configs/` | Профили, их общие фрагменты и инструкции |
| `examples/` | Примеры конфигурации, внешних модулей и сервера MCP |
| `docs/` | Руководства, архитектура и описание проекта |

## Документация

- [Замысел проекта](docs/product/spec.md), [текущее состояние](docs/product/scope.md)
  и [план развития](docs/product/roadmap.md).
- [Настройка моделей и профилей](docs/guides/configuration.md).
- [Приложение и расширения клиента](https://github.com/Proteus-harness/Proteus-desktop/blob/main/README.md).
- [Слоты и модули](docs/architecture/modules.md),
  [архитектура](docs/architecture/architecture.md) и
  [протокол модулей](docs/architecture/process-module-architecture.md).
- [Инструменты и разрешения](docs/guides/security-and-policy.md).
- [История, события и подключение к редакторам](docs/guides/runtime-and-events.md).
- [Разработка и выбор проверок](docs/development/testing.md).

Полный [указатель документации](docs/README.md) помогает найти нужную тему.
Правила работы с кодом — в [AGENTS.md](AGENTS.md).

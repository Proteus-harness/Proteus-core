# Запуск На Другом ПК

Основной клиент — приложение. Готовую папку `Proteus` перенесите целиком
и запустите `proteus-native /path/to/project`; профиль выбирается флагом
`--config` (по умолчанию — основной `config.toml`). Сборка из исходников и системные
зависимости описаны в [руководстве приложения](https://github.com/Proteus-harness/Proteus-desktop/blob/main/README.md).

Сборку Linux x86_64 для Fedora можно скачать из артефакта
`linux-x86_64-release` после успешного запуска workflow
[Check or release Linux portable](https://github.com/Proteus-harness/Proteus-desktop/actions/workflows/release.yml)
с параметром `build`. Проверьте `sha256sum -c SHA256SUMS`, распакуйте
`Proteus-*-linux-x86_64.tar.gz` и запустите `Proteus/install-shortcut`,
если нужен ярлык приложения.

## Архив Настроек

[Скрипт settings-migration.py](https://github.com/Proteus-harness/Proteus-desktop/blob/main/scripts/settings-migration.py)
работает с Python 3.11+. Закройте Desktop перед восстановлением, чтобы открытое
окно не перезаписало сохранённые настройки при выходе.

```bash
# На исходном ПК
python3 settings-migration.py export Proteus-settings.tar.gz
# На новом ПК
python3 settings-migration.py restore Proteus-settings.tar.gz
```

Скрипт переносит профили, prompts, визуальные настройки, привязки и настройки
виджетов. Он корректирует абсолютные пути внутри прежнего домашнего каталога
и сохраняет резервные копии заменяемых файлов. `restore --dry-run` показывает
объём изменений. Архив содержит личные настройки. Авторизация, история чатов
и выбор модели в отдельных чатах остаются на исходном ПК.

Если профили уже перенесены из версии без слота хранения сессий, выполните
`python3 settings-migration.py upgrade-storage` и нажмите «Перезапустить»
в окне Proteus. Скрипт добавляет JSONL к старым TOML-профилям без выбранного
хранилища. Текущая команда `restore` выполняет это обновление при переносе.

## Профили И Доступ К Модели

Именованные профили хранятся в `~/.config/Proteus-harness/configs/`.
Собственные профили можно перенести на новую машину. Пути к секретам должны
соответствовать локальному окружению, например
`$HOME/.config/Proteus-harness/secrets/anthropic.json`.

Ключи и OAuth-авторизация настраиваются на каждом ПК отдельно по
[руководству конфигурации](configuration.md). Секреты не входят в Git
или переносимый пакет приложения.

## Оформление Desktop

Визуальный конфиг не входит в agent TOML. Перенесите
`~/.config/dev.proteus.native/desktop/default.json` и свои отдельные
визуальные JSON/TOML. В каждом окне «Настройки → Конфигурации»
позволяют открыть нужный файл, сделать его default для новых окон или
привязать к текущему файлу агента. Ручной выбор действует только в этом окне;
оформление соседних окон и поведение агента сохраняются.

`~/.config/dev.proteus.native/preferences.json` содержит метаданные приложения,
настройки клавиш, отправки и уведомлений, а также реестр `visual_configs` с
default и привязками. Его можно перенести для сохранения этих настроек,
но пути в реестре абсолютные: если расположение файлов на новом ПК отличается,
выберите default и привязки заново в «Настройки → Конфигурации».
Имена профилей Codex/Pi/OpenCode не задают оформление автоматически.

## Команды Для Терминала

Если нужны CLI и команды авторизации, установите их из репозитория.
Для штатных Pi/OpenCode workers нужны Node.js 22.18+ и npm; установщик
готовит их зависимости в новом snapshot до переключения активной сборки:

```bash
git clone <repo> Agent
cd Agent
./install.sh
proteus --config codex doctor
```

Установщик принимает `PROTEUS_BIN_DIR`, `PROTEUS_HOME` и
`PROTEUS_CONFIG_HOME`. Он хранит `proteus` и самостоятельные
`proteus-<id>-module` (без тестового `proteus-reference-module`)
в `~/.proteus/releases/<snapshot-id>/` и атомарно переключает ссылку
`~/.proteus/current`. Команды в `~/.local/bin` используют выбранную сборку.

Поставляемые профили устанавливаются вместе с CLI. `proteus init coding`
создаёт новую конфигурацию, перезаписывая существующий `config.toml` и
связанные файлы; рабочий профиль выбирайте явно через `--config`.

После настройки подписки ChatGPT:

```bash
proteus-model-pack-module auth openai_codex login
proteus --config codex-subscription modules list
proteus --config codex-subscription tools list
cd /path/to/project
proteus --config codex-subscription "Расскажи о структуре проекта"
```

`proteus` без запроса открывает интерактивный CLI. Сессии и журнал сохраняются
под настроенным корнем конфигурации; при стандартной раскладке это
`~/.config/Proteus-harness/sessions/` и
`~/.config/Proteus-harness/.proteus/events.jsonl`.

Для проверки установщика есть `scripts/install-smoke.sh`: он повторяет
установку во временные каталоги и запускает её проверки. При обычном переносе
готового приложения этот прогон не требуется.

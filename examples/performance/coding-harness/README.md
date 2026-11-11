# Сравнение coding harnesses

Стенд запускает оригинальные Pi, Codex, OpenCode и профили Proteus на одинаковых
публичных задачах. Проверяются готовое решение, завершение процесса, фактические
модель/effort, действия и provider usage. Живые прогоны расходуют квоту модели.
По текущему поручению используется `opencode-go/deepseek-v4.1-flash` с `high`.

[Результаты 11 ноября 2026](../results/2026-11-11-harness-fidelity.md) содержат
также предыдущий срез Sol/Terra и ограничения переносов upstream в слоты.

## Задачи и проверка

| Источник | Задачи | Закреплённая ревизия |
|---|---|---|
| Aider polyglot | `forth`, `poker`, `dominoes`, `variable-length-quantity` | `7e0611e77b54e2dea774cdc0aa00cf9f7ed6144f` |
| Terminal-Bench 2 | `cancel-async-tasks`, `log-summary-date-ranges` | `2fd12b88aafdd04a52c298e3940bcb189f9766d6` |

Polyglot предоставляет Python starter, инструкции и публичные tests. Для TB2
пути `/app/` в задаче заменены workspace-relative путями. В grader сохранено
upstream представление `/app`; Docker Python 3.13 заменён Python 3.13.9 в
bubblewrap. Эти адаптации записаны в `case.json` каждой задачи. Источники и
оригинальные лицензии остаются в закреплённых checkout.

Каждый запуск получает свежие repository и home. Grader работает отдельно с
неизменяемыми копиями tests и исходных logs, без сети. Он принимает результат
при успешных tests и сохранении проверяемых входных файлов. Итоговый PASS также
требует `pipeline_ok` от runner. Начальный starter должен давать FAIL, эталон —
PASS. Подмена публичных tests проверяется отдельным отрицательным контролем.

## Подготовка

Команды выполняются из Proteus-core. Требуются Linux с user namespaces и
`bwrap`, Python 3.11+, Git, `uv`, npm и установленные native CLI. Pi runtime
требует Node.js 22.19+. OpenCode adapter устанавливает закреплённый Linux x64
Node 24.12.0. Standalone CLI и profile adapters устанавливаются отдельно.

```sh
mkdir -p target/performance/coding-harness
git clone --filter=blob:none https://github.com/Aider-AI/polyglot-benchmark \
  target/performance/coding-harness/polyglot
git -C target/performance/coding-harness/polyglot checkout --detach \
  7e0611e77b54e2dea774cdc0aa00cf9f7ed6144f
git clone --filter=blob:none https://github.com/harbor-framework/terminal-bench-2 \
  target/performance/coding-harness/tb2
git -C target/performance/coding-harness/tb2 checkout --detach \
  2fd12b88aafdd04a52c298e3940bcb189f9766d6
uv venv --python 3.13.9 target/performance/coding-harness/grader-venv
uv pip install --python target/performance/coding-harness/grader-venv/bin/python \
  pytest==8.4.1
python3 -B examples/performance/coding-harness/corpus.py prepare
```

`prepare` проверяет source revision и чистоту checkout и требует новый каталог
`cases`. Повторная подготовка использует новый `--output`, для matrix нужен
стандартный `target/performance/coding-harness/cases`.

Для имеющейся portable-поставки установите оба адаптера из Proteus-modules.

```sh
python3 ../Proteus-modules/tool/pi-tools/install.py \
  --prefix ../Proteus-desktop/build/Proteus
python3 ../Proteus-modules/workflow/opencode-runtime/install.py \
  --prefix ../Proteus-desktop/build/Proteus
```

Matrix ищет Pi в `~/.local/bin/pi`, OpenCode в
`~/.opencode/bin/opencode`, Codex в `/usr/lib/chatgpt/resources/codex`, Proteus в
соседней portable-поставке. Одиночный runner ищет native CLI в PATH,
путь также задаётся `--binary`.
Профиль Proteus задаётся `--base-config` внутри `configs/` Core.

Для Flash требуется действующий ключ Go в native OpenCode. Runner получает
только активную запись `opencode-go` через native `auth export`, переносит её в
приватный home запуска и удаляет после извлечения метрик. Значения ключей в
результаты не записываются. Native OpenCode использует собственный приватный
server, импорт авторизации и свой каталог. Перед генерацией проверяется
эффективная привязка основной и title-модели к точному Flash/high.

## Контроли без модели

```sh
python3 -B examples/performance/coding-harness/runner_selfcheck.py
python3 -B examples/performance/coding-harness/adapters_test.py
python3 -B examples/performance/coding-harness/corpus.py grade \
  target/performance/coding-harness/cases/forth \
  --workspace target/performance/coding-harness/cases/forth/repo \
  --output target/performance/coding-harness/forth-starter.json
python3 -B examples/performance/coding-harness/corpus.py grade \
  target/performance/coding-harness/cases/forth \
  --workspace target/performance/coding-harness/cases/forth/repo \
  --reference --output target/performance/coding-harness/forth-reference.json
```

Команда starter ожидаемо возвращает exit 1. Эталон применяется к disposable
копии workspace. Такие же два контроля нужны для остальных пяти задач.
Сохранённые исходные контроли находятся в
`target/performance/coding-harness/corpus-controls.json`.

## Живой запуск

Одиночный запуск профиля Pi с явной моделью и независимой проверкой результата.

```sh
python3 -B examples/performance/coding-harness/runner.py \
  --agent proteus --base-config configs/pi.config.toml \
  --model opencode-go/deepseek-v4.1-flash --reasoning high \
  --case-dir target/performance/coding-harness/cases/forth \
  --output target/performance/coding-harness/one-pi-forth
python3 -B examples/performance/coding-harness/corpus.py grade \
  target/performance/coding-harness/cases/forth \
  --workspace target/performance/coding-harness/one-pi-forth/workspace \
  --output target/performance/coding-harness/one-pi-forth/grading.json
```

Шесть задач и один свежий повтор `variable-length-quantity` для четырёх плеч.

```sh
python3 -B examples/performance/coding-harness/matrix.py \
  --arms pi opencode proteus-pi proteus-opencode \
  --models flash --repeat --jobs 4 \
  --output target/performance/coding-harness/flash-matrix
python3 -B examples/performance/coding-harness/report.py \
  target/performance/coding-harness/flash-matrix \
  --output target/performance/coding-harness/flash-metrics.json
```

`--output` каждого запуска должен быть новым. `--cases` ограничивает corpus.
Матрица записывает pipeline и grading для каждой клетки, сохраняет неуспехи и
возвращает ненулевой exit при любом FAIL. Повтор учитывается отдельно от шести
первых попыток. `report.py` проверяет одинаковые prompt/repository hashes.

`proteus-codex` можно добавить как самостоятельную конфигурацию. Для Flash она
использует Pi model transport и JSON surface настоящего patch engine. Native
Codex с этим endpoint сейчас помечается unavailable после HTTP 403 на Responses
probe. Сопоставимой пары native Codex/Flash в этой конфигурации нет.

## Что зафиксировано

`result.json` хранит CLI command, binary hash, исходные/final file hashes,
модель, effort, exit, timeout и метрики. Рядом лежат prompt, stdout/stderr,
`grading.json`, private runtime без credentials и canonical journal Proteus.
Конфигурация runtime содержит реальные выбранные слоты и benchmark overrides.
Native OpenCode сохраняет registry proof и SQLite usage до очистки private DB.

Input включает cached input, output включает reasoning, когда провайдер
сообщает breakdown. Кэш показан подмножеством input. Отсутствующая категория
сохраняется как unknown. Pi и Proteus считают зарегистрированные model replies;
для Codex и общего числа запросов OpenCode метрика requests остаётся unknown.
OpenCode session usage включает служебные title requests.

Действия Pi, OpenCode и Proteus считаются по tool calls. Native Codex JSON CLI
публикует completed command/file-change items, поэтому его столбец actions имеет
другую гранулярность. Число API-запросов модели из него не выводится.

Agent sandbox разрешает запись в fresh workspace/state, системные mounts
read-only. Сеть доступна модели. Grader имеет лимиты 120 с wall time, 90 с CPU,
2 GiB address space и 8 MiB размера одного файла. Live runner по умолчанию
ограничен 600 с. В сохранённом срезе agent sandbox использует системный
Python 3.14.8, grader — закреплённый Python 3.13.9. Pytest установлен только
в grader environment; polyglot tests
можно запускать через доступный системный `unittest`. Для unattended работы
стенд разрешает выбранные tools внутри внешней OS-песочницы. Это отдельная
настройка относительно интерактивной policy пользовательского профиля.

Запуски идут параллельно, provider cache/server load и фоновые builds могут
влиять на wall time. Текущий набор показывает решения и траектории небольших
Python-задач. Длинные разговоры, большие репозитории, UI, background jobs и
реальная compaction проверяются отдельными сценариями.

## Offline replay Proteus

```sh
python3 -B examples/performance/coding-harness/replay_audit.py \
  --source target/performance/coding-harness/one-pi-forth \
  --output target/performance/coding-harness/replay-one-pi-forth
```

Проверка использует сохранённые model/tool outcomes, `--unshare-net` и
read-only source mounts. `audit.json` содержит matched, количество model/tool
exchanges, issues и hashes до/после. Журнал, session files и workspace должны
сохраниться. При доступном локальном Go credential scanner проверяет его байты
в артефактах без вывода значения, число проверенных ключей записывается явно.
Replay требует ту же версию workflow и модулей, что использовалась при записи.

## Длинные задачи и продолжение сессии

Расширенная серия работает на DeepSeek V4.1 Flash/high. Она использует те же пять
доступных сборок и прежний grader в bubblewrap. На решение задачи, включая все
её пользовательские ходы, выделено 1200 секунд. Эталон и grader работают в
отдельной песочнице. Время grader учитывается отдельно от работы агента.

| Задача | Что проверяет |
|---|---|
| `write-compressor` | Анализ C-декомпрессора и побайтовое восстановление исходного текста из результата размером до 2500 байт |
| `schemelike-metacircular-eval` | Реализация интерпретатора на самом языке, публичные и скрытые программы, выполнение самого интерпретатора |
| `db-wal-recovery` | Ремонт SQLite базы с зашифрованным WAL и экспорт строк. Grader сверяет JSON с чтением самой базы |
| `journal-segments` | Архивирование настоящего журнала Proteus, целостность сегментов и blobs, совместимость существующего читателя |
| `sampler-phase-report` | Потоковая агрегация данных Linux sampler, повторное использование PID, границы фаз, расчёт CPU и интеграция с CLI |
| `ledger-recovery` | Миграция журнала записей, обрыв записи, дедупликация и одновременные процессы |
| `go-review` | Три последовательных запроса в одной сессии. Парсер SGF, подсчёт территории, затем единый API с форматом из первого запроса |

Три задачи взяты из закреплённого Terminal-Bench 2; для Scheme verifier получает до
2400 секунд. Компиляторы и их стандартные библиотеки должны быть доступны в
`/usr`, включая GCC и Rust для reference compressor. Задачи про сегменты и sampler
используют реальные исходники Proteus-core на ревизии `879183f3853ce469346b19634e9440c6d7d7cd0f`
и новые требования. Ledger — отдельная синтетическая кодовая база. Go объединяет
два упражнения из закреплённого Aider polyglot с новой интеграционной задачей.
Для подготовки нужны checkout и grader-venv из раздела «Подготовка».

```sh
suite=examples/performance/coding-harness
data=target/performance/coding-harness
cases="go-review ledger-recovery journal-segments sampler-phase-report write-compressor db-wal-recovery schemelike-metacircular-eval"
python3 -B "$suite/long_corpus.py" $cases --output "$data/long-cases"
python3 -B "$suite/matrix.py" --case-root "$data/long-cases" --cases $cases \
  --arms pi proteus-pi opencode proteus-opencode proteus-codex \
  --models flash --jobs 5 --timeout 1200 --repeat --output "$data/long-flash-reproduction"
```

Каждая подготовленная задача публикуется после двух проверок: starter должен
провалить grader, эталон должен пройти. Проверка записывает оба результата и хэши
grader и исходных файлов. В `go-review` продолжения находятся вне доступной агенту
файловой системы. После каждого хода runner проверяет идентификатор сессии и
число принятых пользовательских сообщений. Финальный grader повторно проверяет
ранние части решения. Отдельный повтор начинается с нового home и workspace.
Число наблюдавшихся compactions и объём входа показываются отдельно: общий input
за несколько запросов может быть больше контекстного окна одного запроса.

Сводка по сорока запускам отдельно показывает решение по grader, завершение
агента, первый результат по каждой задаче и контрольный повтор Go. Полный
расход прерванного запроса может остаться неизвестным. Сумму подтверждённых
токенов таких запусков отчёт показывает как нижнюю границу.

```sh
python3 -B "$suite/long_report.py" --runs "$data/long-flash-reproduction" \
  --case-root "$data/long-cases" --repeat-case go-review \
  --output "$data/long-flash-reproduction.json"
```

[Результаты расширенной серии](../results/2026-11-11-deepseek-long.md) также
содержат отдельный опыт с лимитом выходных токенов в профиле Codex и повторы
после одновременного разрыва соединений. В первичный зачёт входят стартовые
запуски каждой сборки. У компрессора grader сам собирает декодер из эталонного
C source и проверяет декодированный текст и размер результата. Сборка локальной
копии декодера с другими флагами GCC допускается.

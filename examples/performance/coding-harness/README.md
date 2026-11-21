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
Для Proteus имя executable в подготовленной команде берётся из нормализованного
`--binary`, как и путь/hash в metadata: переименованный candidate не заменяется
соседним файлом с именем `proteus`. Это правило построения argv, не проверка
неизменности файла между подготовкой и фактическим запуском.
Профиль Proteus задаётся `--base-config`; прежние fragment-профили должны
находиться внутри `configs/` Core, самодостаточная native-сборка может быть
отдельным TOML-файлом.

Текущая монолитная сборка `--base-config configs/config.toml` определяется по
выбранному именованному model с `provider = "opencode"` и использует нативный
`model/opencode`, без подмены на Pi. Слоты, prompts, hooks и schemas tools
сохраняются; runner разрешает выбранные root tools в своей внешней OS-песочнице,
отключает MCP и записывает эти overrides. Опциональный `--disable-subagents`
отключает роли и collaboration surface только для этого disposable прогона;
без флага роли сохраняются. Native OpenCode с этим флагом получает последний
`subagent deny` и `experimental.subagent_depth = 0`; effective config проверяется
до генерации. Метрики с несколькими journals требуют отдельной агрегации.
Сборка должна быть самодостаточной, без `include`. Provider для этого adapter
подтверждается конфигурацией model binding и наблюдаемым логическим adapter ID
в canonical requests, а не отсутствующим provider ID в ответе внешнего API.

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

## Terminal outcome Native Pi

Для pinned Pi 1.5.2 (`b4afdddf44e059c67ccf5097e5317d913521b470`)
`agent_end` завершает лишь низкоуровневый проход: после него возможны retry,
compaction и continuation. Общий `pi_terminal.py` использует окончательный
assistant из последнего `agent_end.messages` перед `agent_settled` и сбрасывает
его при новой активности. Для clean generation требуются ровно `expected_turns`
settlements, каждый с `stopReason=stop` и `aborted=false`, без незавершённого хвоста.
Прочие и неизвестные причины не считаются clean success этого coding evaluator;
это не универсальное объявление `toolUse` или `deferred` ошибками протокола Pi.

Metrics сохраняют прежние ошибки попыток и usage, отдельно записывают
`pi_outcome` с итоговыми settlements и наблюдавшимися stop reasons. Успешное
восстановление больше не проваливает pipeline только из-за старого error.
Невосстановленный terminal `length` даёт diagnostic `length limit` и остаётся
отдельным от grader PASS. Промежуточный recovered length сохраняется в
`observed_stop_reasons`, но не становится terminal output-limit failure.

Runner по-прежнему требует успешный exit, отсутствие timeout, model/reasoning
verification и continuity. Report fallback использует тот же generation predicate:
это не доказательство успешного CLI postprocessing или полноты всех billable calls.
Полнота generation по-прежнему не гарантирует presence каждой usage category;
проверка observed completion rows описана ниже.
Старый неуспешный artifact без полной settlement boundary теперь даёт unknown.
Сохранённый `pipeline_ok=true` fast path не переоценивается, старые результаты,
оценки и benchmark таблицы автоматически не переписываются.

Локальные synthetic controls, без CLI/API/LLM:

```sh
python3 -B examples/performance/coding-harness/pi_terminal_test.py
python3 -B examples/performance/coding-harness/usage_report_test.py
```

## Coverage наблюдаемых native usage rows

Pi и Codex учитывают usage не только по surviving dict-valued blocks.
`expected_usage_rows` сохраняет число наблюдавшихся assistant `message_end`
для Pi или `turn.completed` для Codex. Это **не** число всех provider requests,
пропавших событий или billable calls; Codex `model_requests` остаётся unknown.
`native_usage` сохраняет прежний список реально найденных dict-valued blocks,
без вставленных вымышленных rows.

Общий `usage_evidence` выдаёт строгий `provider_usage` только если для category
есть integer contribution от каждой expected row. Отсутствующая, null или
non-dict usage row делает full totals unknown, не zero. Отдельно сохраняются
`partial_reported_usage_sums` и per-category `usage_fields_reported_by_rows`.
Нет observation — partial null/count 0; одна row с reported zero — partial 0/count 1.
Для Pi combined input требует `input`, `cacheRead`, `cacheWrite` в одной row;
части разных неполных rows не складываются в выдуманное полное observation.

Report projection из PR126 сохраняет такие partial sums отдельно от full totals.
При ожидаемых nonnegative token counters это observed lower bounds, не доказанная
цена полного run. Exact-int правило не валидирует arbitrary malformed data:
negative integers не исправляются и не запрещаются этим изменением.
Завершение pipeline/generation и полнота token fields остаются разными понятиями.
Valid complete totals и raw usage не меняются; historical files/scores не
пересчитываются автоматически. Исчезнувший целый completion event или auxiliary
usage, которого нет в этом stream, данным cardinality check не обнаруживается.
OpenCode сохраняет прежние значения totals через совместимый `sum_usage` wrapper;
новая expected-event cardinality для него не заявляется.

Focused synthetic check без native CLI/API/LLM:

```sh
python3 -B examples/performance/coding-harness/native_usage_test.py
```

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
Уникальность попытки определяется фактическими model/effort, case, номером
попытки и arm, а не псевдонимом модели в имени папки. Повторное включение одной
попытки отклоняется; конфигурация только с повторами не получает медиану первичных
запусков.

Короткий и длинный отчёты используют одну политику полноты usage. Числовой
subtotal сам по себе не подтверждает завершение generation: для незавершённого
запуска полные `tokens` и итоговые `input`/`cached`/`output` остаются unknown.
Исходные числа сохраняются в `reported_usage`, наблюдаемые нижние границы —
в `observed_partial_usage`. Сводка первичных попыток отдельно содержит
`known_token_runs` и `observed_token_sums`; повторы не входят в эти суммы.
Нулевой observed sum без known runs не доказывает нулевой полный расход.
Флаг `usage_complete` описывает завершение по существующей policy и не означает,
что провайдер сообщил каждую token category. Метрики grader/pipeline, правила
отбора и сохранённые результаты прогонов этим представлением не меняются.

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
JSON и Markdown используют одни выбранные `cases`/`arms`. Повторные оси плана
и дубликаты попыток внутри каждого основного или диагностического набора
отклоняются. Длинный отчёт остаётся серией DeepSeek Flash/high и проверяет
это сочетание у наблюдаемых запусков; исторический baseline учитывается отдельно.
Эти проверки не изменяют сохранённые результаты прежних прогонов.

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

## Обрыв после действия инструмента

[Результаты повторов](../results/reconnect-2026-11-12.md) сопоставляют неуспешные
задачи длинной серии с новыми запусками DeepSeek. Продолжение на копии прежней
сессии проверяется отдельно. Исходные workspace и журнал используются только
для чтения. Синтетический сценарий `recovery_probe.py` проверяет число записей
в файл и соответствующие записи журнала при обрыве во время генерации следующего
tool call. Его можно запускать без подключения к внешней модели.

[Повторы трёх задач Codex после снятия лимита ответа](../results/reconnect-codex-cap-2026-11-12.md)
показывают фактический бюджет в HTTP-запросах DeepSeek и результаты независимой
проверки файлов на момент остановки. Для точного replay старого прогона
`ledger-recovery` с лимитом 16 384 потребовался записанный вариант workflow.
С ним новая сборка воспроизвела все четыре запроса модели и восемь вызовов
инструментов. При подстановке нового workflow replay обнаружил изменившийся
лимит на первом hook `before_context`.

## Сложный публичный набор: Terminal-Bench 4.0

`tb4.py` фиксирует семь задач из upstream tag `v4.0.0`, revision
`452bf305c6daa62fc59061d22133a7cbc7c1572e`. Это все задачи категории Software,
запрашивающие до 2 CPU, 4096 MiB RAM, 10240 MiB storage, без GPU и compose.
Отбор не использует результаты агентов. Получаются `bun-sourcemap-leak`,
`mvcc-lsm-compaction`, `ontology-kg-querying`, `photonic-waveguide-routing`,
`react-lead-form`, `session-window-debug`, `wal-recovery-ordering`.

Исполнение и перенос artifacts выполняет **Harbor 0.26.0**, backend Podman.
Сохраняются исходные Dockerfiles, instruction, tests и отдельный verifier
каждой задачи. Proteus получает только агентскую среду; checkout, эталонные
решения и скрытые tests в неё не копируются. `tb4_agent.py` устанавливает
зависимости агента и переносит готовую portable-сборку с текущим native
`model/opencode`. Авторизация экспортируется приватно из активной Go-записи,
не включается в job config и удаляется после запуска. Canonical sessions,
usage и сведения о выбранной конфигурации остаются в trial `agent/`.

```sh
data=target/performance/tb4-pilot
suite=examples/performance/coding-harness
uv venv --python 3.13.9 target/performance/tb4-harbor-venv
uv pip install --python target/performance/tb4-harbor-venv/bin/python \
  harbor==0.26.0 patchelf==0.19.3.0
# Для установленного Compose V2 frontend нужен rootless Podman socket.
systemctl --user start podman.socket
python3 "$suite/tb4.py" freeze
target/performance/tb4-harbor-venv/bin/python "$suite/tb4.py" run \
  --arm oracle --name oracle-controls
target/performance/tb4-harbor-venv/bin/python "$suite/tb4.py" run \
  --arm nop --name starter-controls
target/performance/tb4-harbor-venv/bin/python "$suite/tb4.py" run \
  --arm inspect --name inspect-bun --only bun-sourcemap-leak --concurrency 1
target/performance/tb4-harbor-venv/bin/python "$suite/tb4.py" run \
  --arm proteus --name proteus-high
```

Checkout размещается в `target/performance/upstream/terminal-bench-4`.
Для экономии места достаточно sparse checkout семи каталогов `tasks/<id>`.
Перед каждым запуском проверяются revision, чистота checkout и полные SHA-256
деревьев задач. Frozen manifest нельзя перезаписать другими условиями; имена
job уникальны, неудачные попытки сохраняются. `oracle` должен дать reward=1,
`nop` — reward=0 без ошибки инфраструктуры. `inspect` не вызывает модель.

Пилот: точный Go model slug `deepseek-v4.1-flash`, effort `high`, одна попытка,
1200 секунд на задачу вместо upstream 28800, без дочерних агентов и без
добавленного лимита ответа. Повторы после инфраструктурных ошибок учитываются
отдельно; автоматических повторов нет. Штатный публичный network default
сохраняется. Сравнение с другим харнесом должно использовать этот же manifest,
grader, доступ к сети, effort, бюджет и запрет делегирования; поддержка native
OpenCode в новом Harbor bridge пока не реализована.

Portable-бинарники, собранные на Fedora, требуют более новой glibc, чем некоторые
upstream images. Bridge копирует библиотеки только агента в `/runtime/abi`
и меняет PT_INTERP/RUNPATH временных ELF-копий через pinned patchelf. Исходная
поставка и библиотеки task tools не меняются. Хеши исходных/временных файлов,
библиотек и patcher сохраняются в trial. `inspect` дополнительно запускает
обычный CLI с EOF: проверяет re-exec app-server без отправки задачи модели.

Это малый срез Software, **не официальный score на всех 66 задачах**.
[Опубликованные DeepSeek 31,2%](https://huggingface.co/deepseek-ai/DeepSeek-V4.1-Flash)
на полном TB4 используют max effort и DeepSeek Harness Minimal. Они показывают
сложность набора, но относятся к раннему checkpoint. Текущий Go catalog называет
модель **1017**; для неё нужны отдельные
[результаты и сведения о расходе](../results/2026-11-16-tb4-reference.md).
Опубликованная серия Claude Code в среднем занимала 84 минуты на попытку:
20-минутный пилот нельзя напрямую сравнивать с её score. Usage/cost estimate
каталога также не доказывает денежное списание по подписке Go.

## Полный Aider Polyglot: парный прогон

`polyglot_corpus.py` сохраняет все 225 задач закреплённого Polyglot checkout:
C++26, Go39, Java47, JavaScript49, Python34, Rust30. Поведение upstream Aider
изучено на commit `5dc9490bb35f9729ef2c95d00a19ccd30c26339c`;
SHA-256 исходных benchmark scripts сохраняются в
`target/performance/polyglot-225/upstream-methodology/provenance.json`.

Агент видит условие, объявленные implementation files и необходимые build
configs. Тесты, `.meta` и примеры решений остаются вне его namespace. Проверка
собирает pristine tree и накладывает только разрешённые solution files. После
первого отказа возвращается диагностический вывод grader; вторая попытка
продолжает ту же native session. Максимум два агентских turn, при первом PASS
второй не запускается. Внутри turn доступны собственные tools и smoke tests,
поэтому это сравнение двух харнесов на задачах Polyglot, а не точная реплика
одной генерации Aider.

```sh
suite=examples/performance/coding-harness
data=target/performance/polyglot-225
python3 "$suite/polyglot_setup.py"
python3 "$suite/polyglot_corpus.py" prepare
python3 "$suite/polyglot_corpus.py" controls --workers 2
python3 "$suite/polyglot_tool_preflight.py" --output "$data/my-tool-preflight"
python3 "$suite/polyglot_run.py" init --output "$data/runs/my-run" \
  --timeout 1200 --budget 20 --tool-preflight "$data/my-tool-preflight/result.json"
python3 "$suite/polyglot_run.py" run --output "$data/runs/my-run" --jobs 4
python3 "$suite/polyglot_report.py" --runs "$data/runs/my-run" \
  --output "$data/runs/my-run/report.json"
```

`setup` закрепляет JDK21, Gradle8.7, JS dependencies, Cargo caches и недостающий
Boost для двух C++ задач; версии и хеши находятся в `toolchains/receipt.json`.
Toolchain/dependency mounts доступны только для чтения. Рабочие кеши Cargo,
Gradle, Go и npm находятся в `/workspace/.benchmark-cache`: эта область
доступна для записи также во внутренней песочнице Proteus и сохраняется между
отдельными tool calls. `/tmp` для этого не подходит: shell создаёт его заново
при каждом вызове. Подготовленный `node_modules` смонтирован для чтения прямо
в workspace JavaScript-задач. Writable state и home
раздельны для каждого task/agent; пользовательские конфиги и секреты в них
не копируются целиком. В disposable Proteus shell component разрешаются только
нужные benchmark toolchain env keys, чтобы условия совпадали с OpenCode.
В отдельном network namespace доступ к HTTPS оставлен только для `opencode.ai`
и `models.opencode.ai` через `polyglot_network.py`; публичные tests/solutions
скачать нельзя. Proxy передаёт TLS непрозрачно и сохраняет только счётчики,
без headers/body/credentials. Ограничение и запрет скачивания tests явно заданы
одинаковым benchmark prompt prefix для обоих харнесов. Стандартные localhost
RPC native OpenCode работают внутри namespace.
Перед новым платным cohort `polyglot_tool_preflight.py --output <new-directory>`
проверяет все шесть языков через текущие workflow/policy/shell slots Proteus.
Только model slot заменён детерминированной fixture: два отдельных
`exec_command` проверяют offline compiler/dependencies, запись и сохранение
cache marker, а также отдельный tmpfs каждого вызова. Платных запросов нет.
Grader не имеет сети и требует наблюдаемого исполнения тестов, а не одного
exit0. Используются Java unskip, Rust include-ignored и все C++ tests; для JS
активируются также 23 `xit` проверки `grep`, пропущенные upstream script.
Эта адаптация сохранена в provenance, два явно optional JS tests остаются skipped.

До платного admission нужны контроли всех задач. У `go--counter` тестов нет:
он отмечен как неоцениваемый и не вызывает модель. Из **224 проверяемых задач**
пять refactor starters уже проходят: Go ledger/markdown, Java ledger/tree-building,
JavaScript ledger. Отчёт дополнительно показывает **219 задач**, на которых
starter не прошёл. Для шести Rust-эталонов используется официальный
`Cargo-example.toml`; receipt отмечает отличающийся reference contract.
Условия и Cargo.toml кандидатских решений сохраняются.

`init` фиксирует source/case/control/prompt hashes, compiler setup, модель и
effort, config, executable/component hashes и digest JS model runtime.
Порядок задач перемешан фиксированным seed, порядок харнесов чередуется.
`run` продолжает отсутствующие задачи и сохраняет неудачные попытки;
неполный trial требует явного аудита. На задачу выделяется 1200 секунд работы
агента суммарно на оба turn; grader180 секунд считается отдельно. Лимит ответа
модели не добавляется. Порог $20 применяется к catalog-equivalent usage:
новые задачи перестают запускаться при его достижении или при неизвестном
usage. Уже выполняющиеся задачи завершаются, поэтому это не жёсткий billing cap.

Отчёт сохраняет first-turn/final PASS, независимый grader PASS, model/effort,
continuity, actions, tokens/cache, стоимость и время. Полнота usage и известная
нижняя граница расхода различаются; cumulative snapshots не суммируются дважды.
Сетевые/квотные ошибки остаются в основной серии; дополнительный clean slice
показывается с собственным знаменателем. Deadline остаётся ошибкой бюджета.
225 языковых вариантов содержат 100 exercise slugs: 95% интервалы строятся
парным cluster bootstrap по упражнению, сохраняя его языковые варианты вместе.
Чужие Aider score/cost относятся к другой модели и протоколу и служат справкой.

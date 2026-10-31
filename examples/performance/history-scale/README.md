# История в каждой из десяти сессий: память, запуск и работа peers

Стенд создаёт настоящие canonical journals через существующий typed
`session_index_fixture`. `--messages 5000 50000 --peers 10` означает **по 5 000
либо 50 000 сообщений в каждой из десяти сессий**, а не суммарно на всех.
Используются только синтетические данные и явные пути к готовым executables.
Python не запускает Cargo, inference внешнего provider или GUI.
Без дополнительных опций вся эта история остаётся активным контекстом модели.
Опция `--model-history-rows 100` задаёт другой сценарий: вся отображаемая
история сохраняется в journal, а активная model history содержит последние
100 исходных canonical сообщений после синтетической compaction.

Два режима измеряют разные границы:

- `resume-idle` — десять независимых настоящих Core HTTP backends. Каждый
  возобновляет свою seeded session, подтверждает `/health` и получает исходный
  session snapshot через SSE. Затем все десять остаются idle 10 секунд,
  завершаются и повторно запускаются на тех же путях с сохранёнными derived
  caches. Это измерение чтения/возобновления сессий; AgentControl здесь нет.
- `agent-control` — настоящий `AgentControlRuntime` с десятью отдельными roles,
  по одному процессу на роль, global parallel/idle cap 10. Выполняются
  spawn/wait и последующие ходы по полученным `task_id`, с fake model, настоящими
  `coding.codex_loop`, `codex_context`, policy и `read_file`. Количество peers
  можно уменьшить для smoke.

## Подготовка executables

Из корня Proteus-core, вне измерения:

```sh
cargo build --locked --release -p proteus-core --bin proteus \
  --example session_index_fixture --example multiagent_perf
```

Reference module собирается отдельно в Proteus-modules. Пары `--binary` и
`--module` должны соответствовать сравниваемой сборке. Один и тот же готовый
`--driver` используется для before/after; он должен включать optional fixture
mode из `../multi-agent/driver.rs`. `--driver` требуется только для
`agent-control`; в `resume-idle` этот executable не запускается. Seeder можно
взять из одной из сравниваемых сборок: общие immutable fixtures создаются один
раз и затем используются в обоих вариантах.
Seeder сохраняет прежний CLI с четырьмя позиционными параметрами; новый
необязательный пятый параметр — число сообщений model history:
`session_index_fixture CONFIG_ROOT WORKSPACE 50000 4 100`. Сначала он записывает
все 50k сообщений, затем выполняет существующий `replace_history` с typed
`changed` compaction report и теми же IDs последних 100 сообщений/parts.
Модель суммаризации не вызывается, token estimates не выдумываются.

## Запуск

Лёгкие проверки без сборки Core и GUI:

```sh
python3 -m unittest discover -s examples/performance/common -p 'test_*.py' -v
python3 -m unittest discover -s examples/performance/history-scale -p 'test_*.py' -v
python3 examples/performance/history-scale/run.py --help
```

Сначала один диагностический повтор. Выберите новые абсолютные пути для
`--output` и `--fixtures`; существующий output не перезаписывается:

```sh
python3 examples/performance/history-scale/run.py \
  --mode resume-idle \
  --binary /absolute/before/bin/proteus \
  --module /absolute/before/bin/proteus-reference-module \
  --seeder /absolute/bin/session_index_fixture \
  --fixtures /absolute/performance/shared-history \
  --output /absolute/performance/before-idle \
  --messages 5000 50000 --peers 10 --repeats 1
```

Для after измените только binary/module и output; оставьте **те же** fixtures,
параметры и workspace. `--seed-base` — другое имя того же `--fixtures`.
Workspace находится внутри общей fixture directory и совпадает между A/B.
Без явного `--fixtures` используется `OUTPUT/fixtures`. Спецификация
существующих fixtures должна точно совпадать с messages/peers/text/payload;
для отдельного smoke с другими параметрами используйте другой путь.
По умолчанию `--repeats 3`; это последовательные повторы на прогретом кэше ОС.
`model_history_rows` также входит в спецификацию fixtures. Compacted и
uncompacted inputs нельзя смешивать: для включения опции выберите новый путь.
Старые fixtures без этого поля продолжают означать uncompacted history.

При ограничении диска можно явно добавить `--link-fixtures` **только для
resume-idle**. В этом режиме journal и blobs имеют hardlinks на общие
canonical inputs; metadata и derived caches остаются отдельными у каждой
копии. После запуска проверяются исходные SHA256 всех canonical файлов.
Любая запись в них делает случай неуспешным и означает, что общие fixtures
больше нельзя считать неизменными. Для активных ходов hardlinks запрещены.
Без этой опции применяется `cp --reflink=auto` с обычным копированием там, где
CoW недоступен. Стенд не удаляет и не подчищает evidence автоматически.

Активный сценарий с теми же размерами истории:

```sh
python3 examples/performance/history-scale/run.py \
  --mode agent-control \
  --binary /absolute/after/bin/proteus \
  --module /absolute/after/bin/proteus-reference-module \
  --seeder /absolute/bin/session_index_fixture \
  --driver /absolute/bin/multiagent_perf \
  --fixtures /absolute/performance/shared-history \
  --output /absolute/performance/after-active \
  --messages 5000 50000 --peers 10 --repeats 1 --rounds 3
```

Для первого функционального smoke: отдельные fixtures, `--messages 100`,
`--peers 1 --repeats 1 --rounds 1 --idle-seconds 1`. Полные 5k/50k измеряются
самостоятельно; smoke не доказывает их успешность.

Чтобы измерить активных peers с полной отображаемой историей 5k/50k и
ограниченным активным контекстом, используйте отдельные compacted fixtures:

```sh
python3 examples/performance/history-scale/run.py \
  --mode agent-control --model-history-rows 100 \
  --binary /absolute/after/bin/proteus \
  --module /absolute/after/bin/proteus-reference-module \
  --seeder /absolute/new/bin/session_index_fixture \
  --driver /absolute/bin/multiagent_perf \
  --fixtures /absolute/performance/shared-compacted-history \
  --output /absolute/performance/after-compacted-active \
  --messages 5000 50000 --peers 10 --repeats 1 --rounds 3
```

Здесь **50k — durable display history, 100 — начальная model history**, а не
50k сообщений в model input. Общий отчёт и каждая строка явно содержат
`history_mode` и `model_history_rows`; seed summaries хранят отдельные counts,
IDs и digests display/model corpora. Следующие turns закономерно расширяют
активную историю; дальнейшие автоматические compactions выключены.

## Проверяемые границы и результаты

Readiness подтверждается session snapshot каждого процесса: точный session ID,
`history.before + transcript.len() == ROWS` и совпадение IDs/текста последних
50 seeded сообщений. Старый полный transcript и новый paged tail принимаются
при одинаковом общем числе сообщений. До/во время замера не запрашивается
полный `/history`. Сохраняются отдельные PID/starttime, время HTTP bind и
snapshot readiness каждого peer, а также время готовности всех десяти.
В `resume-idle` journal и исходные blobs должны остаться побайтно неизменными.
Сводки `/sessions.message_count` и `/conversation.messages` отражают активную
историю и не подтверждают сохранность всех UI rows после compaction.

Для AgentControl `launcher.py` — только benchmark exec adapter. Он принимает
ровно `--config CONFIG --cwd WORKSPACE --new-session server stdio`, проверяет
allowlist config→seed из manifest, заменяет единственный `--new-session` на
`--resume-session SEED` и вызывает `os.execv` настоящего Core. Protocol stdout
не перехватывается. Unit test проверяет сохранение PID при exec и отклонение
дублированных/добавленных flags. Production launch policy не меняется.

В активном режиме hooks/compactor выключены, чтобы seeded corpus не исчез
перед измерением. Opt-in compaction создаётся seeder заранее и сохраняется
как часть canonical fixture; runtime hooks не меняются.
`codex_context.max_context_bytes` ограничивает только
добавленные context chunks, а не историю. Fake model не является настоящим
provider с поддержкой такого context window: этот сценарий проверяет затраты
локального runtime на большую canonical history. Существующие transport frame
limits остаются штатными; большой случай может честно завершиться ошибкой
ещё до model/tool round trip. Такая ошибка сохраняется как **failed**, вся
серия получает failed и exit code 1. Успешный read-only запуск не подменяет
успех активного сценария.

После успешных активных ходов отдельный нетаймируемый verifier проверяет
неизменность исходного journal prefix и blobs, число/IDs/текст seeded сообщений
во всех model requests и checkpoints, отсутствие новых compactions, ожидаемые settled
turns, два fake model calls на ход и фактическое пересечение ожиданий peers.
Каждый выполненный turn проходит workflow replay; comparison должен matched,
source journal — неизменным. JSONL читается по записи/blob, а не целиком в
память процесса измерения.

В compacted варианте каждый model request должен содержать все исходные
100 tail messages без изменений и не содержать старые compacted-away IDs.
До измерения отдельный HTTP preflight проверяет UI count и исходные head/tail
IDs/текст; этот этап прогревает derived read caches. После turns и replay
ещё один свежий HTTP runtime подтверждает `snapshot`/`/history/page.total`
не меньше исходного числа display rows и проверяет исходные первые/последние
страницы. Эти probes выполняются вне sampled RAM/CPU интервала и сохраняют
свои результаты. Исходный journal prefix и blobs также остаются неизменными;
plain history replacement, сбрасывающий UI transcript, отвергается.

Каждый случай сохраняет configs, manifest, launcher/driver events, stderr,
gzip исходных SSE snapshots, `samples.jsonl`, `measurement.json`, verification,
replay logs и `report.json`. Общий `invocation.json` содержит аргументы,
SHA256 executables/источников и allocator environment. На ошибке остаются
partial samples, `failure.log` и failed report; следующие случаи выполняются
отдельно. Папка общих fixtures хранит исходные canonical файлы и seed summaries.

Память/CPU/IO снимает `../common/proc_sampler.py`. Основные показатели относятся
только к actual Core backends и reference modules; Python supervisor/exec
adapter и Rust driver явно сгруппированы как controller и не входят в
`subject`. Полное дерево также сохранено. RSS повторно считает shared pages;
PSS+SwapPss делит их пропорционально. Отдельно есть SwapPss, private память,
sampled peaks, resident medians и CPU weighted mean по длительности интервалов
(100% — одно ядро). `VmHWM` разных процессов не складывается в пик дерева.

Первые запуски используют свежие копии canonical inputs, но кэш ОС не
очищается: это не cold boot и не измерение allocation count. Restart сохраняет
derived files именно своего случая. Sequential `/proc` scan не атомарен;
короткие пики, короткоживущие дети и финальные CPU/IO между последней выборкой
и exit могут быть пропущены. Измерение не доказывает поведение GUI, production
provider или качество ответов модели.

По умолчанию интервал — 200 мс, resident idle — 10 секунд на этап, timeout
случая — 600 секунд, предел subject PSS+SwapPss — 6 ГиБ. `--timeout`,
`--max-pss-mib`, `--interval`, `--idle-seconds`, `--rounds` задаются явно.
Timeout и memory limit сохраняют partial evidence и завершают только
наблюдённые собственные PID с совпадающим starttime; уничтожения по имени
процесса нет. HOME не переопределяется; служебные Proteus/XDG paths изолированы.

## Локализация Пика Полного Журнала

Число сообщений не ограничивает объём journal: сохранённые model requests
могут многократно содержать одну большую историю. Для отдельного read-only
сценария есть `--seed-model-requests N --request-payload-kib K`. Новый typed
seeder добавляет N auxiliary request/response pairs после исходной истории.
Каждый request содержит неизменённую активную историю и ещё одно System
сообщение ровно K×1024 ASCII bytes. Exchange IDs уникальны; окончательная
canonical/display history не пополняется. Это проверка хранения и чтения
журнала, без вызова модели и без утверждения о workflow replay.

Размеры ограничены: N 0..128, K 0..8192 KiB, дополнительный payload до 1 GiB;
каждый сериализованный request также ограничен 64 MiB. Начинайте с 16×1024 KiB
и одного процесса, увеличивайте только после анализа первого результата.
Лимит N×K учитывает только дополнительное System сообщение. Повторённая
active history входит в каждый request сверх этого объёма: это не предел
общего размера journal или расхода RAM при seed validation.
Этот корпус разрешён только в `resume-idle`; активные turns используют
обычные fixtures и отдельный verifier. Спецификация, SHA и counts сохраняются,
совпадение payload и пар exchange проверяется независимо по journal/blob.
Обычные старые fixtures означают N=K=0.

Для внутренних фаз есть opt-in Cargo feature `memory-diagnostics`.
Изменение typed seeder требует сборки именно этого example; диагностические
метки требуют отдельного Core. Desktop, renderer extensions и runtime
modules для этого не собираются. Используйте готовый reference module:

```sh
CARGO_BUILD_JOBS=2 cargo build --locked --release -p proteus-core \
  --features memory-diagnostics --bin proteus --example session_index_fixture

python3 examples/performance/history-scale/run.py \
  --mode resume-idle --memory-trace \
  --binary /absolute/diagnostic/proteus \
  --module /absolute/ready/proteus-reference-module \
  --seeder /absolute/diagnostic/session_index_fixture \
  --output /absolute/new-output --fixtures /absolute/new-fixtures \
  --messages 216 --peers 1 --repeats 1 --idle-seconds 3 --interval .05 \
  --seed-model-requests 16 --request-payload-kib 1024 --link-fixtures
```

`--memory-trace` задаёт `PROTEUS_MEMORY_TRACE=OUTPUT/memory-trace.jsonl` только
измеряемым runtime subprocesses. Создание seed и нетаймируемые verification,
replay/UI probes получают environment без этой переменной. Наличие feature
в готовом binary runner автоматически не определяет: проверяйте, что trace
создан и содержит PID измеренного backend. Без feature production runtime
вообще не компилирует logger или probe calls; runtime flags/wire не меняются.

JSONL содержит static phase, PID, scope/parent IDs, `wall_time_ns` для
сопоставления с samples, process-local monotonic `elapsed_us`, RSS/VmHWM и
`glibc_allocator_totals_proxy`: arena/uordblks/fordblks/hblkhd. Это общие totals
glibc, не профиль владельцев Rust objects; mmap учитывается отдельно hblkhd.
Нет содержимого сообщений, аргументов tools или путей сессий. Метки стоят
между полным чтением, validation, model-context/render, SQLite save, page
selection/drop и восстановлением/освобождением runtime history. У async
release отдельные point events; синхронные parent scopes не пересекают await.
Запись меток влияет на время и немного на allocation, поэтому trace нужен
для локализации фаз, а latency сравнивайте также без logger на том же binary.

Для проверки формы уже существующего журнала есть отдельный read-only
`clone_read.py`. Он создаёт приватную копию canonical `session.json`,
`journal.jsonl` и blobs, без derived SQLite/cache. Исходник не открывается
runtime на запись. Используется только текущий session/journal format;
старые форматы не преобразуются. Указанный в metadata workspace должен
существовать. Source spec храните локально с правами 0600 вне tracked files:

```json
{"source_session_dir":"/absolute/session/directory","source_identity":"opaque-label"}
```

```sh
python3 examples/performance/history-scale/clone_read.py \
  --source-spec /absolute/private-source.json \
  --binary /absolute/diagnostic/proteus \
  --module /absolute/ready/proteus-reference-module \
  --output /absolute/new-private-output --memory-trace \
  --idle-seconds 2 --interval .05 --max-pss-mib 3072 --timeout 180
```

Первый запуск строит read model заново, restart сохраняет созданные derived
файлы; фактическое попадание в cache проверяйте по phase trace.
Оба работают с одним backend и synthetic profile без hooks/tools, модель
не вызывается. HTTP ограничен loopback health/events/shutdown. Snapshot
проверяется в памяти: session identity, counts, transcript digest и отсутствие
активного execution. В отчёте остаются hashes/counts, не текст сообщений.
SHA canonical файлов проверяется до/после чтения и у источника, и у копии.
Output имеет права 0700: внутри остаются сама копия и приватный stderr,
поэтому его нельзя публиковать целиком. Убедитесь, что source spec и output
исключены из Git. Установленный Core этот стенд не заменяет.

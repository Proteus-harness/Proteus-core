# Измерение производительности

Постоянные стенды хранятся здесь; новые сценарии расширяют их, а не создают
одноразовые скрипты в `/tmp`. Синтетические fixtures, логи и отчёты остаются
генерируемыми данными под `target/performance/` или явным `--output`.
Пользовательские сессии и сетевые модели для этих замеров не нужны.

Зафиксированный baseline: [замеры 28 октября 2026](results/2026-10-28.md)
с параметрами, версиями, памятью, запуском и пересборкой. Требуются Linux и
Python 3.11+; измеряемые процессы должны разрешать чтение своих `/proc` данных.

| Стенд | Что измеряет |
|---|---|
| [history-scale](history-scale/README.md) | 5 000/50 000 настоящих сообщений **в каждой** из десяти сессий: восстановление HTTP peers и отдельный сценарий AgentControl |
| [multi-agent](multi-agent/README.md) | Конкурентные `AgentControlRuntime` peers, реальные tools, локальная fake model, resident pool, повторные ходы и replay |
| [native-history](../../../Proteus-desktop/examples/performance/native-history/README.md) | Окно GPUI, backend и descendants: первое открытие длинной истории и повторный запуск |
| [journal-recovery](journal-recovery/README.md) | Восстановление журнала с многократной заменой одной history; это **не** разговор из 50 000 сообщений |
| [session-db](session-db/README.md) | Изолированный прототип чтения JSONL/SQLite/Turso, не текущий runtime Proteus |
| [common](common/README.md) | Общий Linux `/proc` sampler для RSS/PSS/SwapPss, CPU, IO, faults и этапов запуска |
| [build-iteration](build-iteration/README.md) | No-op и повторяемая пересборка после изменения исходника, с восстановлением файла и сохранением Cargo cache |
| [summarize.py](summarize.py) | Сравнение существующих JSON-отчётов до/после в новом Markdown-файле |

Ссылка на native-history предполагает соседние checkout Proteus-core и
Proteus-desktop. Он также доступен в
[репозитории Desktop](https://github.com/Proteus-harness/Proteus-desktop/tree/main/examples/performance/native-history).

## Сборка и запуск

Из корня Proteus-core подготовьте Core и существующие typed drivers:

```sh
CARGO_BUILD_JOBS=2 cargo build --locked --release -p proteus-core \
  --bin proteus --example session_index_fixture --example multiagent_perf
python3 scripts/build_module.py --profile release
```

Последняя команда печатает фактический путь к reference-модулю; используйте
его в `--module`. Для сравнения сохраните executables каждой версии и их
ревизии до следующей сборки. Пути ниже — абсолютные пути к этим сохранённым
файлам. Driver и seeder оставляют одинаковыми между A/B, если сравнивают именно
дочерние Core/module; изменение самого driver требует отдельного сравнения.

Десять восстановленных HTTP-сессий, в каждой по 5 000 или 50 000 сообщений:

```sh
python3 examples/performance/history-scale/run.py \
  --mode resume-idle \
  --binary /absolute/bin/proteus --module /absolute/bin/proteus-reference-module \
  --seeder /absolute/bin/session_index_fixture --driver /absolute/bin/multiagent_perf \
  --messages 5000 50000 --peers 10 --repeats 3 \
  --fixtures target/performance/history-fixtures \
  --output target/performance/history-before
```

Для реального agent-control запуска используйте `--mode agent-control` с
новым `--output`. HTTP resume-idle проверяет read-only восстановление и idle;
AgentControl запускает peers через runtime и выполняет ходы; явный benchmark
launcher направляет их в заранее подготовленные сессии. Эти режимы отвечают
на разные вопросы и выводятся раздельно. Для `after` передайте другую пару
Core/module, тот же `--fixtures` и новый каталог результата.

Конкурентная работа десяти peers без заранее заполненных длинных историй:

```sh
python3 examples/performance/multi-agent/run.py \
  --driver /absolute/bin/multiagent_perf \
  --binary /absolute/bin/proteus --module /absolute/bin/proteus-reference-module \
  --counts 10 --rounds 3 --repeats 3 --idle-cap 10 \
  --payload-kib 32 --delay-ms 500 \
  --output target/performance/multi-agent-before
```

Для Desktop обычная поставка собирается командой
`python3 scripts/portable.py --no-archive` из Proteus-desktop. Подготовка
изолированного дисплея и точный native CLI описаны в native-history README.
`--fast` использует другой профиль клиента: результаты `release` и `fast`
сравнивайте раздельно. Не запускайте сборку параллельно с runtime-замерами.
Время no-op Cargo, пересборки после изменения и полной сборки указывайте
раздельно; сохраняйте incremental caches и фиксируйте профиль, число jobs,
wall/CPU time и максимальный RSS процесса сборки.

Сводка готовых `report.json` без новых замеров; передавайте только имеющиеся серии.
Флаги `--idle-*` и `--active-*` можно повторять для нескольких отчётов; output должен быть новым:

```sh
python3 examples/performance/summarize.py \
  --native-before /absolute/native-before/report.json --native-after /absolute/native-after/report.json \
  --idle-before /absolute/idle-before/report.json --idle-after /absolute/idle-after/report.json \
  --active-before /absolute/active-before/report.json --active-after /absolute/active-after/report.json \
  --build-before /absolute/build-before/report.json --build-after /absolute/build-after/report.json \
  --output target/performance/comparison.md
```

## Как читать результаты

- Записывайте ревизии и SHA256 executables, параметры корпуса, число сообщений
  на сессию, фактическое число peers, readiness condition и условия сборки.
  Три повтора дают медиану и разброс; один запуск — диагностический результат.
- RSS повторно считает общие страницы. PSS распределяет их между процессами;
  рядом нужен SwapPss и PSS + SwapPss, чтобы вытеснение в swap не выглядело
  экономией памяти. Суммируйте процессы одного дерева в одной выборке;
  sampled peak — нижняя оценка, сумма индивидуальных VmHWM не является пиком.
- «Первый запуск» здесь означает новый процесс без derived app caches;
  «повторный» — новый процесс с сохранёнными caches того же случая. Файловый
  кэш ОС остаётся прогретым. Это не cold boot; сброс системного кэша не нужен.
- HTTP readiness, получение snapshot, создание окна и отображение transcript
  — разные события. Сравнивайте одинаковое событие и одинаковую фазу idle;
  CPU 100% означает одно логическое ядро.
- Проверки результата и исходных fixture hashes обязательны. Read-only запуск
  сохраняет canonical journal побайтно; активные ходы проверяют ожидаемые
  добавления, сохранность исходной истории и replay. Performance numbers не
  являются assertions. Ошибка, timeout или нарушение этих проверок остаются
  неуспешным случаем, без вывода об ускорении.

# Ресурсы реальных process peers

Синтетический сценарий для сравнения одинаковых сборок до и после изменения
runtime. Родительский Rust driver использует публичный `AgentControlRuntime`:
запускает настоящие `proteus server stdio`, ждёт результаты и возобновляет те же
задачи по `task_id`. Python только готовит данные, снимает Linux `/proc` и после
измерения проверяет журналы. GUI, inference родителя, реальные документы,
credentials и сетевые модели в сценарий не входят.

```sh
CARGO_BUILD_JOBS=2 cargo build --locked --release -p proteus-core --example multiagent_perf
python3 examples/performance/multi-agent/run.py \
  --binary /absolute/before/bin/proteus \
  --module /absolute/before/bin/proteus-reference-module \
  --output target/performance/multi-agent/before
```

Для `after` передайте другую пару executables и новый `--output`, оставив
**тот же driver**, параметры и входные данные. Скрипт сохраняет SHA256 всех трёх
executables, полный config, payload, события этапов, выборки и результаты replay.
Он отказывается перезаписывать существующую папку случая.

По умолчанию запускаются 1, 5 и 10 peers, по три последовательных хода каждого:

- отдельные Rust-компоненты `model`, `workflow`, `context`, `capabilities` у
  каждого peer и у родительского registry;
- настоящий `coding.codex_loop`, `codex_context`, `codex_compact`, policy и
  `read_file`; модель — поставляемый `fake`;
- `read_file` читает неизменный файл 32 КиБ; fake model возвращает его
  содержимое после реального tool round trip, поэтому история растёт;
- задержка fake stream — 500 мс на слово. В большом payload нет пробелов,
  поэтому размер файла не превращается в тысячи искусственных задержек;
- readiness отмечается по перенаправленному `ToolCallRequested(read_file)`:
  он подтверждает, что настоящая fake model уже выдала tool call. Parent runner
  не перенаправляет текстовые deltas. Все peers должны пройти эту точку до
  первого terminal result; event log дополнительно подтверждает максимум
  одновременно ожидающих model calls и ровно два model calls на ход;
- затем измеряются resident idle и teardown. Журнал каждой сессии содержит
  ожидаемое число settled turns; каждый ход проходит workflow replay с явным
  `--turn-id` без изменения source.

Лимиты global/role выставлены в 10. Resident cap по умолчанию равен числу peers,
чтобы все задачи можно было продолжить во втором и третьем ходе. Это **явное
условие измерения**, а не штатный cap 8. Нулевой cap можно измерить отдельно,
без resume:

```sh
python3 examples/performance/multi-agent/run.py \
  --binary /absolute/bin/proteus --module /absolute/bin/proteus-reference-module \
  --output target/performance/multi-agent/no-resident \
  --counts 1 5 10 --rounds 1 --idle-cap 0
```

Для короткой проверки: `--counts 1 --rounds 1`. Для повторов:
`--repeats 3`. `--payload-kib`, `--delay-ms` и `--rounds` задают размер сценария.
Уменьшение задержки может нарушить требование действительной конкурентности;
такой случай завершится ошибкой и не будет помечен успешным.

Показатели:

- сумма **PSS + SwapPss** всех принадлежащих driver процессов, sampled peak,
  resident median и память driver после teardown;
- CPU seconds, minor/major faults и последние наблюдавшиеся `/proc/PID/io`
  counters всех процессов, включая завершившиеся;
- отметки времени до каждого model-produced tool call, полного readiness и settlement;
- размер и digest canonical journals, результат replay.

Выборки по умолчанию идут раз в 200 мс. Sampled peak — нижняя оценка пика;
последние counters могут пропустить финальные операции короткоживущего процесса.
`wchar` включает запись в pipes и логи, а `write_bytes` описывает kernel storage
accounting. Это разные показатели; ни один не является числом allocations.
Резерв стека не считается resident memory. `VmHWM` отдельных процессов не
складывается в «одновременный пик».

При PSS + SwapPss выше 4 ГиБ сценарий прерывается; лимит можно задать через
`--max-pss-mib`. При ошибке завершаются только наблюдавшиеся собственные процессы
с проверкой PID/start time. HOME и пользовательские config/data находятся в
папке случая. По умолчанию выполняется один диагностический повтор на прогретом
кэше ОС; он не даёт статистического доказательства ускорения и не измеряет
качество настоящего provider или полный desktop startup.

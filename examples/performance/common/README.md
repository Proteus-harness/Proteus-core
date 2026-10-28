# Общие инструменты измерения ресурсов Linux

`proc_sampler.py` использует только стандартную библиотеку Python. Импорт не
запускает процессы или потоки, не устанавливает обработчики сигналов и не
изменяет глобальное состояние процесса. Production runtime не затрагивается.

Из корня Proteus-core:

```python
from examples.performance.common import Tree, Recorder

# Создать сразу после Popen, пока root ещё существует.
tree = Tree(process.pid, root_group="gui", classifier=my_classifier)
with Recorder(tree, "run.jsonl") as log:
    log.mark("startup", variant="before")
    log.sample()
    # Launcher проверяет свою readiness condition и снимает выборки ~100 мс.
    log.mark("ready")
    log.sample()
    # ... загрузить синтетическую историю и продолжить выборки ...
    log.mark("settled_5000")
    sample = log.sample()
summary = tree.totals()
```

`classifier(row)` возвращает `gui`, `backend`, `modules`, `extensions` либо
собственную группу. В строке доступны `pid`, `parent`, `exe` из `/proc/PID/exe`,
`argv` и `name`. Для root всегда используется `root_group`, для детей —
классификатор. По умолчанию применяются консервативные правила для имён
executable и argv[0]. Для интерпретаторов и общих launcher нужны явные правила
по argv/PID. Для каждого отдельного запуска, включая relaunch, создавайте новый
`Tree`. `sample(phase=None)` можно использовать напрямую без `Recorder`.

CLI для собственного, заранее запущенного процесса:

```sh
python3 examples/performance/common/proc_sampler.py PID \
  --output run.jsonl --interval 0.1 --duration 20
```

Каждый JSONL `sample` содержит RSS/PSS/SwapPss/private в КиБ для живых процессов,
суммы по группам и по всему дереву. `rss_kib` повторно учитывает общие страницы;
`pss_swap_kib` складывает PSS и SwapPss. Поле итоговой сводки
`sampled_simultaneous_peaks_kib` берёт максимум **суммы всего дерева в одной
выборке**, а не сумму индивидуальных пиков. `VmHWM` сохраняется отдельно для
каждой process identity и никогда не складывается в пик дерева.
`mark(phase, **details)` записывает монотонное и календарное время; последующие
выборки несут это имя этапа. Каждый JSONL event сразу сбрасывается в файл.

Root закрепляется по PID + starttime из `/proc/PID/stat`. Обнаруженные дети
остаются принадлежащими дереву после переподчинения; сохранённый PID принимается
только при совпадении starttime. После чтения процесса identity проверяется
повторно для защиты от повторного использования PID во время выборки.
Исчезнувшие процессы дают **ноль текущей памяти**, но их последние наблюдённые
CPU/IO counters остаются в итогах. Zombies также дают ноль памяти. Ошибки чтения
памяти перечислены в `memory_incomplete_pids`; суммы неполной выборки — нижняя
оценка. Обход последовательный, поэтому не является атомарным kernel snapshot;
`scan_duration_seconds` показывает длительность обхода. Краткие пики между
выборками могут быть пропущены.

CPU percent вычисляется за интервал как
`(utime + stime) / CLK_TCK / elapsed * 100`; 100% — одно логическое ядро.
`cutime/cstime` не учитываются, чтобы не удваивать CPU детей. У первой выборки
нет baseline. Новые дети, родившиеся внутри интервала, считаются от нуля;
более ранние процессы без достоверного baseline указаны в
`cpu_missing_baseline_pids`. `interval_counters` также содержит read/write bytes,
число символов, syscall counters и minor/major faults. Если процесс либо counter
не прочитан в предыдущей выборке, следующая delta пропускается и перечисляется
в `counter_missing_baselines` процесса, чтобы старый baseline не раздувал
показатель одного интервала. Накопительные counters сохраняют последнее
успешное наблюдение при временных ошибках чтения.

`read_bytes`/`write_bytes` — kernel storage IO, а `rchar`/`wchar` учитывают и
кэшированные операции. **Финальные CPU/IO между последней выборкой и выходом,
а также дети, целиком родившиеся и завершившиеся между выборками, не
учитываются.** Потомок, переподчинённый до первого обнаружения, также может быть
пропущен. Для полного lifetime accounting нужны более частые выборки либо
cgroup/taskstats. Этот инструмент не очищает кэши и не определяет, что запуск
«холодный»: startup/readiness и cold/warm/relaunch задаёт внешний launcher.

Лёгкие проверки из корня репозитория:

```sh
python3 -m unittest discover -s examples/performance/common -p 'test_*.py' -v
```

Они проверяют PID reuse, сохранение переподчинённых детей, пики суммарной
памяти, JSONL phases и восстановление после ошибки IO. Живая синтетика выделяет
16 МиБ в двух собственных Python-процессах; GUI и существующие процессы
пользователя не используются.

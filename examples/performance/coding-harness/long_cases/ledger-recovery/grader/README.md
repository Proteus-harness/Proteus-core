# Проверки OS-контракта Ledger

Две проверки `os.replace`/`os.fsync` выполняются через `io_probe.py` в новом
Python-процессе с тем же interpreter и solution cwd, что у grader. Wrapper
устанавливается до первого импорта `ledger`: обычный direct import или
сохранение функции в default argument не должны обходить instrumentation.
Имена внутренних файлов реализации не используются.

Ошибка replace включается только после подготовки исходного состояния;
проверяются её достижимость, распространение, прежние bytes всех файлов
(кроме lock) и чтение последнего значения. Сбор fsync очищается после open
и между append/compact. Проверка по-прежнему требует file sync при append,
не менее двух file sync и directory sync при compact. Это не полное
доказательство crash safety или порядка всех syscalls.

Остальные test bodies и публичные release inputs не меняются. Child stderr
и stdout доступны в assertion родительского теста. Отдельный локальный
timeout не вводится: общий budget grader не переопределяется. Новый subprocess
не является новой изоляцией среды.

Локальные проверки самого probe без моделей и настоящего решения Ledger:

```sh
python3 -B examples/performance/coding-harness/ledger_probe_test.py -v
```

Они запускают настоящий probe на синтетическом JSON Store с qualified,
direct и default-captured imports, проверяют отбраковку отсутствующих sync и
оставленных temporary files, включая `-O`. Это не полный штатный grader и
не доказательство корректности reference/submitted реализации Ledger.

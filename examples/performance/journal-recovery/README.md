# Холодное восстановление журнала

Изолированный стенд для находки `PQ-04` из
[proteus-modules-dot](https://github.com/Qweasd123tg/proteus-modules-dot/blob/main/research/catalog.json).
Использует публичный `SessionStore`, без models, modules, сети и пользовательских
данных. Все output directories должны быть новыми; исходные данные сохраняются.

Два фиксированных корпуса по 64 MiB payload: 512 замен history по 128 KiB
(inline) и 128 замен по 512 KiB (blob). Повторяется одна идентичная canonical
message: текущая history мала, а старые snapshots остаются в журнале. Это
намеренно выделяет цену восстановления superseded records, а не моделирует
полный разговор или среднюю рабочую нагрузку.

- `recover`: свежий process открывает существующую сессию и делает один durable
  append. Writer остаётся жив две секунды для замера памяти.
- `projection`: свежий process строит и удерживает полную strict projection.
- `verify`: отдельный process проверяет точные message/part identities и
  содержимое, число records, revision и последовательность. Его RSS/время не
  входят в замер операции. Seed hash остаётся неизменным; projection не пишет.

По три свежих process/copy на каждый режим. `/usr/bin/time` даёт kernel peak RSS,
а `/proc` — settled PSS+SwapPss. Пиковый RSS включает открытие SessionStore;
reported latency начинается после него. Allocator glibc с arenas=2, тёплый
файловый кеш. Графика, поведение моделей и fsync tail latency не измеряются;
результат не является уменьшением idle RAM всего клиента.

```sh
cargo build --locked --release -p proteus-core --example journal_recovery_perf
# Сохранить baseline executable до изменения Core.
python3 examples/performance/journal-recovery/run.py \
  --binary /absolute/baseline/probe --seeds /fresh/owned/seeds \
  --output /fresh/owned/before --create-seeds
# После сборки изменённого Core использовать те же seeds и другой output.
python3 examples/performance/journal-recovery/run.py \
  --binary /absolute/after/probe --seeds /existing/owned/seeds \
  --output /fresh/owned/after
```

Нужно около 2 GiB для сохранённых корпусов/копий двух вариантов, плюс Cargo
cache/build space. Runner имеет deadline и завершает только свою process group.
Performance numbers не являются тестовыми assertions; correctness checks
обязательны для каждого замера. Группировка commit и durable-before-effect
барьеры не меняются.

# Pi, Codex, OpenCode и профили Proteus

Завершены 91 выбранный trial на шести Python-задачах, все прошли pipeline и
независимый grader. Это 56 предыдущих запусков Sol/Terra и 35 запусков
DeepSeek Flash. Реальные upstream реализации Pi и OpenCode подключены через
слоты Proteus, canonical replay повторяет записанные model/tool exchanges.
Полный перенос поведения Codex имеет конкретные открытые границы.

В первом срезе Sol/high потратил 109 382 входных токена в native Pi и
110 409 в Proteus Pi. На Flash суммарный расход разошёлся сильнее из-за
траектории одной asyncio-задачи. Числа и найденные несовпадения приведены ниже.

[Данные по каждому измеренному запуску](2026-11-11-harness-runs.json) сохраняют
PASS, actions и usage для 56 GPT и 35 выбранных Flash trials, включая
контрольные повторы. Native OpenCode pilot вынесен из этой выборки.

## Набор и условия

Четыре Python exercises взяты из Aider polyglot на commit
`7e0611e77b54e2dea774cdc0aa00cf9f7ed6144f`, ещё две задачи из Terminal-Bench 2
на `2fd12b88aafdd04a52c298e3940bcb189f9766d6`. Задачи проверяют стековый
интерпретатор, ранжирование покерных рук, цепь домино, VLQ, отмену asyncio tasks
и агрегацию logs по диапазонам дат. TB2 адаптирован к workspace-relative путям и
Python 3.13.9 в bubblewrap; исходный grader сохранён. Каждый starter дал FAIL,
каждый эталон — PASS, подмена публичных tests отклонена. Live shell использует
системный Python 3.14.8, grader — 3.13.9. Версия interpreter может влиять на
проверку asyncio cancellation, её нужно учитывать при разборе ошибки.
[Polyglot source](https://github.com/Aider-AI/polyglot-benchmark/tree/7e0611e77b54e2dea774cdc0aa00cf9f7ed6144f),
[Terminal-Bench source](https://github.com/harbor-framework/terminal-bench-2/tree/2fd12b88aafdd04a52c298e3940bcb189f9766d6).

Для каждого плеча создавались свежие workspace/home и одинаковый task prompt.
Внешние плагины, DCP, MCP и пользовательские skills исключены из eval config.
Встроенные native plugins OpenCode остаются частью его runtime. Итоговый PASS
требует независимого grader, успешного завершения CLI и наблюдаемых точных
модели/effort. В записанных Proteus cohorts общий deadline — 600 с, лимит
одного model request — 180 с. Текущий runner уравнял model deadline с общим
600 с; сохранённые конфигурации описывают фактически измеренный вариант.
Input включает кэш, reasoning входит в output. Денежная стоимость
по этим числам не рассчитывалась. Команды, limits и правила подсчёта описаны в
[README стенда](../coding-harness/README.md).

Версии первого среза — Pi 1.5.2, Codex CLI 0.167.0 и OpenCode 2.0.22.
Native executables и Core binary записаны с SHA256 в каждом `result.json`.
Core в этих запусках имеет binary SHA256
`167b4de1a8978e3cbb9cc983b9c8739be1253dcab740f0a6c4782246ecbd4fb2`.

## Предыдущий срез Sol и Terra

Завершены 56 запусков. Это шесть задач плюс один свежий повтор VLQ, две модели
и четыре плеча. Все 56 прошли pipeline и grader, поэтому качество упёрлось в потолок
этого небольшого набора. Таблица суммирует только
первые шесть попыток, все восемь контрольных повторов также прошли.

| Основная модель / effort | Harness | PASS | Actions | Input | Cached input | Output | Model requests |
|---|---|---:|---:|---:|---:|---:|---:|
| Sol 6.1 / high | native Pi | 6/6 | 37 | 109 382 | 70 144 | 6 226 | 33 |
| Sol 6.1 / high | Proteus Pi, первый model adapter | 6/6 | 34 | 110 409 | 70 016 | 5 373 | 30 |
| Sol 6.1 / high | native Codex | 6/6 | 33 | 387 540 | 349 056 | 7 515 | unknown |
| Sol 6.1 / high | native OpenCode | 6/6 | 45 | 209 277 | 123 136 | 7 039 | unknown |
| Terra 6.1 / xhigh | native Pi | 6/6 | 38 | 125 385 | 76 416 | 5 092 | 32 |
| Terra 6.1 / xhigh | Proteus Pi, первый model adapter | 6/6 | 40 | 116 417 | 74 752 | 4 864 | 32 |
| Terra 6.1 / xhigh | native Codex | 6/6 | 37 | 416 184 | 339 328 | 7 316 | unknown |
| Terra 6.1 / xhigh | native OpenCode | 6/6 | 54 | 203 839 | 146 048 | 6 347 | unknown |

Native Codex actions обозначают command/file-change items CLI. Для остальных
плеч actions обозначают tool calls, их гранулярность отличается. Requests у
Codex неизвестны, OpenCode total usage включает служебные title requests. В первом GPT-срезе
эффективная auxiliary model отдельно не проверялась.
Первый Proteus Pi уже использовал upstream tools, prompt, loop и compactor,
но model transport был прежним Proteus OpenAI adapter. Позднее model/v13
переведён на настоящий Pi ModelRuntime. Эти версии разделены в артефактах.

Суммы повторно строятся из `target/performance/coding-harness/vanilla/`.
Сохранённая сводка — `target/performance/coding-harness/vanilla-metrics.json`.
Входных токенов у Proteus Pi относительно native Pi на Sol больше на 0,94%,
на Terra меньше на 7,15%. Сходство сумм согласуется с близким поведением на этом
наборе. Статистическая устойчивость и полная equivalence требуют других
нагрузок, особенно длинного контекста и отмены.

Точный `gpt-6.1-luna` недоступен в подключённой Codex-подписке. Проверка
возвратила provider error; отдельное плечо Luna 6.1 отсутствует. Следующие
живые эксперименты переведены на DeepSeek по ограничению квоты владельца.

## DeepSeek Flash

Рабочая привязка — `opencode-go/deepseek-v4.1-flash` с effort `high`.
Для Proteus используется Pi model transport с явным `opencode-go`; Pi loop и
compactor берут descriptor из того же provider catalog. Native OpenCode перед
генерацией проверяет эффективные main/title bindings.

Первые native OpenCode Flash-запуски подтвердили основную модель, но не модель
служебного заголовка. Они сохраняются как pilot с неизвестной auxiliary route.
Для дальнейшей выборки введена явная `agents.title.model` и проверка `config.get`.
Установка `small_model` для этой версии не давала подтверждённой привязки.

Завершены 35 выбранных запусков, все прошли pipeline и grader. В каждом
из пяти плеч решены шесть задач и один свежий повтор VLQ. Все семь выбранных
native OpenCode trials имеют подтверждённые main/title bindings. Ранние шесть
native OpenCode trials исключены по отсутствию этой проверки независимо от
успеха задачи, их результаты сохранены.

| Harness | PASS первых попыток | Tool calls | Input | Cached input | Output | Model requests |
|---|---:|---:|---:|---:|---:|---:|
| native Pi | 6/6 | 48 | 420 440 | 344 832 | 31 868 | 45 |
| Proteus Pi | 6/6 | 52 | 318 422 | 257 280 | 23 773 | 47 |
| native OpenCode | 6/6 | 65 | 801 804 | 682 496 | 32 104 | unknown |
| Proteus OpenCode, зафиксированный runtime | 6/6 | 60 | 507 165 | 423 040 | 42 120 | 49 |
| Proteus Codex, Flash overrides | 6/6 | 64 | 588 330 | 504 448 | 30 149 | 48 |

Таблица снова суммирует шесть первых попыток. Контрольные повторы всех пяти
плеч прошли и остаются отдельными строками исходных данных. Сводка строится
этой командой.

```sh
python3 -B examples/performance/coding-harness/report.py \
  target/performance/coding-harness/flash-native-matrix-01 \
  target/performance/coding-harness/flash-native-opencode-verified-matrix-02 \
  target/performance/coding-harness/flash-native-opencode-verified-matrix-03 \
  target/performance/coding-harness/flash-proteus-pi-codex-matrix-01 \
  target/performance/coding-harness/flash-opencode-proteus-01 \
  --output target/performance/coding-harness/flash-verified-metrics.json
```

`report.py` исключает native OpenCode Flash-запуски без подтверждённой title
привязки до проверки дубликатов и сохраняет путь/причину в `excluded`.

На Flash у Proteus Pi суммарный input меньше на 102 018 токенов. Asyncio-задача
дала снижение на 149 475, остальные пять вместе потребовали на 47 457 больше.
Model requests и tool calls суммарно выросли. Поэтому снижение общей суммы
объясняется конкретной траекторией решения. На свежем VLQ input native Pi
изменился с 23 987 до 18 688, native OpenCode — с 51 818 до 94 417. Один повтор
уже показывает заметную вариативность даже при фиксированном task.

Зафиксированный Proteus OpenCode runtime получил восемь schema rejections
на optional null arguments и один отказ неинтерактивной policy на `rm -rf`
в семи trials. Модель восстановилась и решила задачи. Текущий adapter исправлен
после этой серии, её usage остаётся привязанным к прежней версии. Отдельный
post-fix Flash VLQ smoke дал PASS 26/26, 5 model requests, 8 tool calls,
23 496 input, 16 640 cached input и 1 082 output без recorded failures.
Артефакт — `target/performance/coding-harness/flash-opencode-proteus-fixed-smoke-01`.

Native Codex через Go Responses probe получил HTTP 403. Это конкретный
результат проверенного endpoint. Flash-профиль Proteus Codex использует
JSON surface настоящего patch engine и Pi model transport. Его результат
показывает работу этой сборки при явном override `codex_patch.freeform=false`.
Парного native Codex/Flash результата для этого endpoint нет.

Финальная установка также прошла два offline replay с отключённой сетью и read-only
workspace. Для профиля Pi совпали 6 обращений к модели и 7 вызовов инструментов,
для исправленного OpenCode — 5 и 8. В обоих случаях SHA256 исходного журнала
сохранился. Отдельная проверка Pi на Flash после переноса provider metadata
прошла 26/26 тестов VLQ без ошибок инструментов. Данные находятся в
`target/performance/coding-harness/replay-audit-01/{pi-final-installed,opencode-final-installed}/audit.json`.

## Какие части действительно перенесены

| Профиль | Прямо используемый upstream | Непокрытая граница |
|---|---|---|
| Pi | Четыре tool definitions/handlers, генератор prompt, загрузка project context, agent loop, compaction и ModelRuntime из закреплённого Pi 1.5.2 | UI tool updates, полный ExtensionContext и Pi session environment в bash |
| OpenCode | `@opencode/core@2.0.22`, native system-prompt plugins, AGENTS/env discovery, read/write/edit/shell/glob/grep, input repair, `SessionStep`, `SessionCompaction.layer` и добавленный после matrix native patch selection | Полная tool registry, background jobs, native retry/overflow/recovery, remote provider compaction, formatter registrations и полный session DB/instruction-delta lifecycle |
| Codex | Публичные model-specific base instructions rust-v0.167.0, настоящий patch engine и grammar, underlying tools | Серверный model-catalog prompt drift, native persistent exec/wait, causal nested operation/replay и native remote compaction |

Pi исходно отклонял `read.offset=null`, хотя native Pi предварительно
применяет свой argument validator с coercion. Адаптер сохранял исходный model
call и ошибочно передавал эти исходные args в Core. Исправление связывает
исполняемые args после настоящих upstream preparation/validation до checkpoint.
Raw assistant call сохраняется, Core проверяет policy и исполнение по этому
явному binding. Дифференциальный тест и свежий Flash VLQ прошли.

OpenCode имеет аналогичную границу перед Core schema validation. В первом
перенесённом Flash smoke модель восстановилась после ошибок `shell.timeout=null`
и `glob.hidden=null`. Исправленный adapter применяет upstream ToolInputRepairPlugin к
исполняемому call до checkpoint. Matrix и post-fix smoke имеют отдельные
версии артефактов. После smoke также уточнён порядок запуска native tool fibers
по sequential/parallel metadata; это изменение проверено локальными regressions.

Для Codex сравнение уже показало реальное расхождение. Публичный закреплённый
Sol template имеет 21 769 символов, эффективный template native catalog —
21 524. Native CLI вызывал `exec`, публичная сборка Proteus выставляет underlying
tools напрямую. Прототип native bridge доказал время жизни turn-local cells,
но arbitrary JavaScript и его nested calls требуют корректных causal bindings
и replay outcomes. `full_codex_fidelity_passed=false` записан в
`target/performance/coding-harness/codex-slot-check/differential.json`.

## Проверка canonical replay

| Профиль / сохранённый запуск | Model exchanges | Tool calls | Matched |
|---|---:|---:|---|
| Proteus Pi / VLQ | 6/6 | 8/8 | true |
| Proteus Pi / Forth | 7/7 | 8/8 | true |
| Proteus OpenCode / smoke | 6/6 | 10/10 | true |

Значения взяты из сохранённых `audit.json` в
`target/performance/coding-harness/replay-audit-01/`. Replay запущен без сети и
с read-only source mounts, model/tool outcomes взяты из журнала. Hashes
workspace, session files и journal сохранены. В source artifacts отсутствовали
credential files, scanner не обнаружил байтов проверенного активного Go key.
Это проверяет повторяемость записанной сборки и целостность журналирования.

## Как соотносятся чужие результаты

У Ibragim `core10-v1.3` содержит десять личных задач, по три запуска. Версии
Pi 0.85.1 и Codex 0.118.0 отличаются от здешних. Для Sol/medium опубликованы
$0.0704 за задачу в Pi и $0.1375 в Codex. Репозиторий evals-from-sessions
поставляет skill для подготовки собственных Harbor evals. Эти опубликованные
цены относятся к его задачам и окружению.
[Лидерборд](https://ibragim.dev/leaderboard/),
[skill](https://github.com/ibragim-bad/evals-from-sessions),
[описание методики](https://teletype.in/@ibragim_bad/fm5jSJvYb_5).

Composio использовал 30 hard agentic tasks против реальных приложений через
hosted MCP router, DeepSeek V4 Pro (0813)/max. Pi решил 21/30, Codex 20/30,
средние total tokens составили 924 990 и 383 722. Это другая нагрузка и модель.
Фиксированный небольшой prompt Pi и число дальнейших действий влияют на расход
совместно, поэтому оба опубликованных наблюдения совместимы с разными corpus.
[Исследование Composio](https://composio.dev/content/pi-agent-vs-codex).

# Более длинные задачи на DeepSeek Flash

Проверено 40 из 40 запланированных запусков. Pipeline и независимый grader прошли 23. Для запусков выбраны opencode-go/deepseek-v4.1-flash и effort high.

Дополнительно сохранены 2 проверки output cap и 5 свежих повторов после коррелированных socket failures. Они приведены отдельными таблицами и сохраняют первоначальные оценки.

Таблица суммирует первые попытки. Контрольный повтор учитывается отдельными строками в JSON. Input включает cache read, output включает reasoning. Полное число запросов OpenCode неизвестно, его primary requests видны в assistant records, total usage включает служебные обращения.

| Агент | Grader PASS | Полный PASS / проверено | Тулы | Запросы | Primary requests | Input | Cached input | Output | Медиана с |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| pi | 4 | 4/7 | 169 | 153 | 153 | ≥6 615 605 | ≥6 213 632 | ≥261 367 | 648.31 |
| proteus-pi | 5 | 4/7 | 126 | 117 | 117 | ≥3 841 795 | ≥3 533 824 | ≥202 284 | 256.561 |
| opencode | 6 | 3/7 | 182 | — | 161 | ≥6 750 535 | ≥6 323 584 | ≥243 088 | 601.932 |
| proteus-opencode | 5 | 5/7 | 167 | 130 | 130 | ≥5 244 549 | ≥4 829 568 | ≥257 153 | 353.105 |
| proteus-codex | 4 | 3/7 | 156 | 120 | 120 | ≥5 570 351 | ≥5 260 288 | ≥118 744 | 296.952 |

| Задача | Требование | Starter FAIL | Gold PASS | Файлов в starter |
|---|---|---|---|---:|
| db-wal-recovery | Восстановление SQLite WAL и сверка БД с recovered.json | True | True | 2 |
| write-compressor | Анализ C-декодера и сжатие исходного текста до 2500 байт | True | True | 3 |
| schemelike-metacircular-eval | Интерпретатор Scheme, исполняющий тестовые программы и себя | True | True | 34 |
| journal-segments | Архивирование настоящего canonical journal и совместимость consumers | True | True | 12 |
| sampler-phase-report | Агрегация process sampler с PID reuse, фазами и взвешенным CPU | True | True | 12 |
| ledger-recovery | Многофайловое хранилище с crash recovery, миграцией и concurrent writers | True | True | 14 |
| go-review | Три последовательных запроса на SGF parser, territory counting и интеграцию | True | True | 6 |

В WAL grader добавлена независимая проверка реального чтения восстановленной БД и совпадения всех строк с recovered.json. В compressor сохраняются исходные data.txt и decomp.c, generated ELF decomp разрешено перекомпилировать. Authoritative grader заново собирает декодер из исходного C и сравнивает байты результата. Первоначальная защита hash ELF давала ложное отклонение при перекомпиляции. Обе коррекции применены одинаково ко всем пяти результатам задачи без нового обращения к модели. Прежние grades сохранены рядом с обновлёнными. Journal suite закреплён на исходных 37 тестах серии.

Знак ≥ обозначает сумму только зарегистрированного usage при отсутствующем полном total. Пропущенные token fields остаются неизвестными. JSON хранит число полных наблюдений и сумму доступных значений, чтобы незавершённый или частично записанный запуск сохранялся в учёте.

| Задача | Pi | Proteus Pi | OpenCode | Proteus OpenCode | Proteus Codex |
|---|---|---|---|---|---|
| db-wal-recovery | PASS, 6 тулов, 48.536 с | PASS, 7 тулов, 53.145 с | PASS, 27 тулов, 161.234 с | PASS, 12 тулов, 98.889 с | PASS, 6 тулов, 69.585 с |
| write-compressor | PASS, 12 тулов, 648.31 с | PASS, 13 тулов, 248.648 с | PASS, 14 тулов, 571.513 с | PASS, 13 тулов, 342.108 с | PASS, 24 тулов, 296.952 с |
| schemelike-metacircular-eval | TIMEOUT, 29 тулов, 1 200.003 с | FAIL, 6 тулов, 167.884 с | TASK PASS / CLI FAIL, 41 тулов, 602.928 с | FAIL, 8 тулов, 168.895 с | TASK PASS / CLI FAIL, 42 тулов, 1 145.613 с |
| journal-segments | TIMEOUT, 32 тулов, 1 200.004 с | FAIL, 27 тулов, 633.105 с | FAIL, 30 тулов, 602.35 с | TIMEOUT, 33 тулов, 1 200.006 с | FAIL, 8 тулов, 387.814 с |
| sampler-phase-report | PASS, 28 тулов, 466.507 с | TASK PASS / CLI FAIL, 20 тулов, 477.487 с | TASK PASS / CLI FAIL, 25 тулов, 601.932 с | PASS, 32 тулов, 477.161 с | FAIL, 12 тулов, 208.22 с |
| ledger-recovery | FAIL, 39 тулов, 880.443 с | PASS, 30 тулов, 961.882 с | TASK PASS / CLI FAIL, 21 тулов, 602.954 с | PASS, 42 тулов, 1 048.287 с | FAIL, 7 тулов, 216.817 с |
| go-review | PASS, 23 тулов, 201.865 с | PASS, 23 тулов, 256.561 с | PASS, 24 тулов, 309.028 с | PASS, 27 тулов, 353.105 с | PASS, 57 тулов, 542.591 с |

Свежий повтор go-review использует те же три пользовательских запроса в новой сессии.

| Агент | Полный PASS сначала / повтор | Тулы сначала / повтор | Input сначала / повтор | Continuity повтора |
|---|---|---:|---:|---|
| pi | True / True | 23 / 16 | 260 551 / 288 623 | True |
| proteus-pi | True / False | 23 / 23 | 388 738 / 371 855 | True |
| opencode | True / True | 24 / 26 | 499 183 / 487 354 | True |
| proteus-opencode | True / True | 27 / 19 | 496 828 / 281 564 | True |
| proteus-codex | True / True | 57 / 41 | 1 682 557 / 834 059 | True |

| Задача и попытка | Агент | Причина | Не пройденные требования |
|---|---|---|---|
| schemelike-metacircular-eval / 1 | pi | agent_deadline, solution_rejected_by_tests | test_interp |
| schemelike-metacircular-eval / 1 | proteus-pi | transport_or_cli_failure, solution_rejected_by_tests | test_interp |
| schemelike-metacircular-eval / 1 | opencode | transport_or_cli_failure | Решение прошло grader, процесс завершился ошибкой |
| schemelike-metacircular-eval / 1 | proteus-opencode | transport_or_cli_failure, solution_rejected_by_tests | test_interp |
| schemelike-metacircular-eval / 1 | proteus-codex | transport_or_cli_failure | Решение прошло grader, процесс завершился ошибкой |
| journal-segments / 1 | pi | agent_deadline, solution_rejected_by_tests | Запись legacy без обязательного завершающего LF принята, хотя контракт требует ValueError. |
| journal-segments / 1 | proteus-pi | solution_rejected_by_tests | Запись legacy без обязательного завершающего LF принята, хотя контракт требует ValueError. |
| journal-segments / 1 | opencode | transport_or_cli_failure, solution_rejected_by_tests | Запись legacy без обязательного завершающего LF принята, хотя контракт требует ValueError. |
| journal-segments / 1 | proteus-opencode | agent_deadline, solution_rejected_by_tests | Запись legacy без обязательного завершающего LF принята, хотя контракт требует ValueError. |
| journal-segments / 1 | proteus-codex | model_output_limit, solution_rejected_by_tests | filesystem_path_validation, journal_format_validation, compatibility_regression, input_validation |
| sampler-phase-report / 1 | proteus-pi | transport_or_cli_failure | Решение прошло grader, процесс завершился ошибкой |
| sampler-phase-report / 1 | opencode | transport_or_cli_failure | Решение прошло grader, процесс завершился ошибкой |
| sampler-phase-report / 1 | proteus-codex | transport_or_cli_failure, solution_rejected_by_tests | journal_format_validation, concurrency_or_deduplication, process_identity, sampler_aggregation, input_validation |
| ledger-recovery / 1 | pi | solution_rejected_by_tests | Циклическое значение Python вызвало RecursionError вместо публичного LedgerError. |
| ledger-recovery / 1 | opencode | transport_or_cli_failure | Решение прошло grader, процесс завершился ошибкой |
| ledger-recovery / 1 | proteus-codex | model_output_limit, solution_rejected_by_tests | atomicity_or_recovery, journal_format_validation, concurrency_or_deduplication, sampler_aggregation, compatibility_regression, input_validation |
| go-review / 2 | proteus-pi | solution_rejected_by_tests | Интеграция вернула неправильные territory counts при успешных отдельных parser и counting tests. |

JSON содержит имена всех упавших tests. Частично записанное решение проверяется тем же grader после timeout или transport failure. Совпадение этой проверки с PASS не меняет факт ошибки процесса.

В 4 первичных native OpenCode trials UnsupportedContentType появился через 599.343–599.969 с после первого assistant message. Плановый внешний deadline равен 1200 с, outer timed_out во всех этих записях false. Точные timestamps и возраст последнего model request сохранены в JSON. Источник этого ограничения по данным серии не установлен.

В первичной серии 5 ошибок UND_ERR_SOCKET зарегистрированы в интервале 0.012 с. Близость времени — наблюдение, конкретный источник обрыва не установлен. Для заранее выбранных пяти клеток выполнен один свежий повтор с тем же заданием и бюджетом.

| Задача | Агент | Полный PASS повтора | Grader PASS | Тулы | Запросы | Секунды |
|---|---|---|---|---:|---:|---:|
| sampler-phase-report | proteus-codex | False | False | 30 | 20 | 369.528 |
| sampler-phase-report | proteus-pi | False | True | 22 | 18 | 368.721 |
| schemelike-metacircular-eval | proteus-codex | False | True | 25 | 15 | 368.768 |
| schemelike-metacircular-eval | proteus-opencode | False | True | 14 | 11 | 1 200.002 |
| schemelike-metacircular-eval | proteus-pi | False | True | 9 | 9 | 367.746 |

Первичные результаты, timeout, code failures и отдельные cap controls сохраняют собственные оценки. Повторы не заменяют их в сводной таблице первых попыток.

Отдельные записанные ходы проверены offline replay.

| Запись | Model exchanges | Tool calls | Matched | Workspace и session сохранены |
|---|---:|---:|---|---|
| target/performance/coding-harness/long-replay-01/go-review-pi-turn3/audit.json | 10/10 | 9/9 | True | True |

Дополнительная проверка output cap проводится в свежих сессиях с тем же заданием и cap 384000. Эти результаты учитываются отдельно от первых попыток.

| Задача | Полный PASS | Input | Output | Запросы | Тулы | Секунды |
|---|---|---:|---:|---:|---:|---:|
| journal-segments | False | ≥154 428 | ≥24 277 | 8 | 17 | 531.083 |
| ledger-recovery | False | ≥15 950 | ≥962 | 3 | 6 | 529.372 |

Сравнение первых canonical requests подтвердило неизменность материалов, tools, модели и effort при замене cap во всех 2/2 контролях. Последующая траектория модели может различаться. Ошибки контрольных запусков и полнота usage сохранены отдельно в JSON.

Подготовлены независимые starter/gold controls. Состояние каждого и hashes grader сохранены в JSON. Отказ grader и ошибка CLI отдельно влияют на итоговый PASS. Native Codex через этот Go endpoint ранее получил HTTP 403, поэтому здесь проверяется только профиль Proteus Codex с явно указанным Chat Completions transport и JSON patch surface.

| Агент | Медиана тулов, прежние 6 | Медиана тулов, новый набор | Медиана секунд, прежние 6 | Медиана секунд, новый набор |
|---|---:|---:|---:|---:|
| pi | 7.5 | 28 | 60.06 | 648.31 |
| proteus-pi | 8.5 | 20 | 58.31 | 256.561 |
| opencode | 8.0 | 25 | 70.052 | 601.932 |
| proteus-opencode | 9.0 | 27 | 70.972 | 353.105 |
| proteus-codex | 11.5 | 12 | 86.754 | 296.952 |

В 37 из 40 первичных записей известен compaction count, зарегистрировано 0 compactions. Остальные counts неизвестны. Provider context window равен 1 000 000 токенов, давление на полное окно в этой серии не достигнуто.

Прежний набор и расширенный набор содержат разные задачи. OpenCode adapter после прежнего среза получил исправления argument preparation и scheduling. Разность медиан описывает эти наблюдаемые серии, причинный эффект одной сложности отдельно не измерялся.

Canonical requests содержат output limits [16384, 384000]. Зарегистрировано 2 model failures из-за length limit. Descriptor или лимит native модели и реально запрошенный cap приведены отдельно в JSON каждого trial. Такие отказы учитываются в первом результате, дальнейшая проверка с другими лимитами образует отдельный эксперимент.

Наибольший зарегистрированный input одного запроса — 146 090 токенов. Это provider usage для полного входа запроса. Compaction берётся из canonical history, Pi session entries или отдельно сохранённого native OpenCode count. Отсутствующее поле OpenCode помечено неизвестным.

Автоматический аудит отметил 0 потенциальных обращений к сети или внешнему grader. Поля аргументов не публикуются, проверяемые вызовы обозначены hashes. Срабатывание требует просмотра соответствующего synthetic trace.

[Данные всех запусков](2026-11-11-deepseek-long.json). Прогоны использовали fresh home/workspace и локальные native runtime versions из предыдущего сравнения. Плановый общий deadline задачи равен 1200 с. Точные команды находятся в result.json каждого запуска. Время включает запуск и завершение агента, независимый grader выполняется после него. Измерения сделаны при параллельных запусках и зависят от нагрузки provider.

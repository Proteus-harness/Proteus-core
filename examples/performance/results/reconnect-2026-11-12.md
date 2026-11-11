# Продолжение Proteus после обрыва

Повторены 10 прежних неудачных запусков на DeepSeek V4.1 Flash с effort high. Каждый запуск получает чистый рабочий каталог и лимит 20 минут. Для каждой задачи сохранены исходные инструкции, профиль и номер повтора. В основной таблице указаны результаты исправленного проверяющего.

| Задача | Профиль | Повтор | Процесс завершён | Тесты пройдены | Время | Действия | Запросы |
|---|---|---:|---|---|---:|---:|---:|
| schemelike-metacircular-eval | proteus-pi | 1 | нет | да | 1 200 с | 12 | 11 |
| schemelike-metacircular-eval | proteus-opencode | 1 | нет | нет | 1 200 с | 34 | 34 |
| schemelike-metacircular-eval | proteus-codex | 1 | нет | нет | 598 с | 4 | 4 |
| journal-segments | proteus-pi | 1 | нет | нет | 1 200 с | 26 | 23 |
| journal-segments | proteus-opencode | 1 | нет | нет | 1 200 с | 30 | 31 |
| journal-segments | proteus-codex | 1 | нет | нет | 204 с | 11 | 5 |
| sampler-phase-report | proteus-pi | 1 | да | да | 545 с | 37 | 32 |
| sampler-phase-report | proteus-codex | 1 | да | да | 989 с | 40 | 25 |
| ledger-recovery | proteus-codex | 1 | нет | нет | 197 с | 8 | 4 |
| go-review | proteus-pi | 2 | да | да | 169 с | 10 | 11 |

Из 10 завершённых запусков 3 прошли процесс и тесты. Успешных проверок кода без завершённого процесса: 1. Успешная проверка кода после остановки по времени не считается завершённым процессом.

Отдельно прежние 5 запусков с обрывом сокета: завершились и прошли тесты 2/5, только тесты — 1/5. Новых реальных сетевых обрывов в этих повторах: 0.

В чистых повторах не наблюдалось сетевых обрывов: изменения оценок сами по себе не измеряют эффект автоматического retry. Контролируемая проверка обрыва описана отдельно в [recovery-proof](../coding-harness/recovery-proof.md).

Причины незавершённых запусков:

- schemelike-metacircular-eval / proteus-pi: лимит 20 минут.
- schemelike-metacircular-eval / proteus-opencode: лимит 20 минут, тесты не пройдены.
- schemelike-metacircular-eval / proteus-codex: лимит длины ответа, тесты не пройдены.
- journal-segments / proteus-pi: лимит 20 минут, тесты не пройдены.
- journal-segments / proteus-opencode: лимит 20 минут, тесты не пройдены.
- journal-segments / proteus-codex: лимит длины ответа, тесты не пройдены.
- ledger-recovery / proteus-codex: лимит длины ответа, тесты не пройдены.

Лимит длины сработал в 3 запусках профиля Codex при неизменённом ограничении 16 384 токена на ответ. Восстановление сети не исправляет этот отказ.

Полный расход токенов известен только для завершённых ответов. Знак ≥ обозначает нижнюю границу; неизвестные токены оборванных ответов не заменены нулями.

| Задача / профиль | Полный расход известен | Входные токены | Выходные токены |
|---|---|---:|---:|
| schemelike-metacircular-eval / proteus-pi | нет | ≥285881 | ≥16653 |
| schemelike-metacircular-eval / proteus-opencode | нет | ≥1663019 | ≥38865 |
| schemelike-metacircular-eval / proteus-codex | нет | ≥83288 | ≥9334 |
| journal-segments / proteus-pi | нет | ≥1243887 | ≥54115 |
| journal-segments / proteus-opencode | нет | ≥1598948 | ≥53510 |
| journal-segments / proteus-codex | нет | ≥51417 | ≥2454 |
| sampler-phase-report / proteus-pi | да | 1679398 | 43814 |
| sampler-phase-report / proteus-codex | да | 1125046 | 47665 |
| ledger-recovery / proteus-codex | нет | ≥24279 | ≥1529 |
| go-review / proteus-pi | да | 145820 | 15028 |

Установленная программа и runtime совпали с начальным снимком: да. Инструкции, исходные файлы и model/effort сверены для каждого запуска.

Продолжение сохранённой сессии использует копию ранее упавшего sampler/ProteusPi. Процесс завершён: да, тесты пройдены: да. Старая история сохранилась: да, исходная сессия неизменна: да, повторно исполненных прежних call IDs 0.
Это отдельный запуск: 244 с, 7 запросов модели, 9 действий. Его расход не включён в итоги чистых повторов.
Первый callable grade ошибочно сравнил абсолютный fixture path с относительным workspace. Оба пути нормализованы в corpus.grade; тот же frozen output перепроверен без model calls. Первый grading JSON сохранён отдельно.

Воспроизведение нового turn совпало с журналом: да, запросы модели 7/7, действия 9/9. Replay выполнялся с отключённой сетью и read-only исходными файлами.

Парный offline regrade исправляет только регистрацию импортируемого модуля в sys.modules. Это позволяет стандартному dataclass читать строковые annotations. Grader выполнялся с временной копии; исходные решения, prompt, grader и защищённые файлы сохранены. Новых запросов модели: 0. Исходные оценки сохранены в следующей таблице и JSON.

| Задача | Профиль | Старый grader до / после | Исправленный grader до / после |
|---|---|---|---|
| schemelike-metacircular-eval | proteus-pi | нет / да | нет / да |
| schemelike-metacircular-eval | proteus-opencode | нет / нет | нет / нет |
| schemelike-metacircular-eval | proteus-codex | да / нет | да / нет |
| journal-segments | proteus-pi | нет / нет | нет / нет |
| journal-segments | proteus-opencode | нет / нет | нет / нет |
| journal-segments | proteus-codex | нет / нет | нет / нет |
| sampler-phase-report | proteus-pi | да / нет | да / да |
| sampler-phase-report | proteus-codex | нет / да | нет / да |
| ledger-recovery | proteus-codex | нет / нет | нет / нет |
| go-review | proteus-pi | нет / да | нет / да |

По исправленному grader: 4/10 решений прошли тесты, 3/10 прошли одновременно pipeline и тесты. Изменения оценок controls/native/cold перечислены в JSON audit.

История инструментов в retry подтверждается записанными canonical requests и call IDs. Перезапрос генерации до принятого terminal response повторяет текущую попытку модели. Нативный OpenCode также умеет сохранять частичный assistant и продолжать новым сообщением. Этот дополнительный native lifecycle остаётся отдельной границей совместимости.

Pi 1.5.2 реализует автоматическое продолжение в AgentSession с тремя паузами 2, 4 и 8 секунд. [AgentSession](https://github.com/earendil-works/pi/blob/v1.5.2/packages/coding-agent/src/core/agent-session.ts), [настройки](https://github.com/earendil-works/pi/blob/v1.5.2/packages/coding-agent/src/core/settings-defaults.ts).

OpenCode различает Retry и Continue в runner, общая policy допускает до десяти повторов. [Runner](https://github.com/anomalyco/opencode/blob/527f0b931d1f9b3ebd34e106c51b31ce5db5b075/packages/core/src/session/runner/llm.ts), [policy](https://github.com/anomalyco/opencode/blob/527f0b931d1f9b3ebd34e106c51b31ce5db5b075/packages/core/src/session/runner/retry.ts).

Codex 0.167.0 по умолчанию допускает пять восстановлений стрима и пересобирает запрос из принятой истории. [Sampling](https://github.com/openai/codex/blob/086ccc833bc9cf01024ad2dd6af7dbd421b6bf9a/codex-rs/core/src/session/turn.rs), [recovery](https://github.com/openai/codex/blob/086ccc833bc9cf01024ad2dd6af7dbd421b6bf9a/codex-rs/core/src/responses_retry.rs).

Claude Code при --resume восстанавливает завершённые tool calls и пропускает вызов, который выполнялся во время обрыва. [Ошибки](https://code.claude.com/docs/en/errors#the-response-above-may-be-incomplete), [восстановление сессии](https://code.claude.com/docs/en/sessions#what-a-resumed-session-restores).

В официальных release notes Droid v0.231.0 описано отбрасывание незавершённых calls, а v0.242.0 добавляет retries для усечённых streams. [Release notes](https://docs.factory.com/changelog/release-notes). Эти описания не подтверждают универсальный exactly-once и не являются сравнением производительности.

Полнота usage и фактически наблюдавшиеся reconnect exchanges находятся в JSON. Новый семантически одинаковый вызов с новым ID может быть намеренным действием модели. Для незаписанного внешнего эффекта после аварии процесса сохранение истории не доказывает единственность исполнения.

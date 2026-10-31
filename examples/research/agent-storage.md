# Журналы сессий: Codex, Rust OC, OpenCode и Pi

Сравнение исходников на 31 октября 2026 года. Это исследование механизмов,
а не описание уже внедрённого формата Proteus или решение о миграции.
Сборки, inference и сравнительные runtime-замеры этих агентов не выполнялись;
пользовательские сессии и credentials не читались. Локальные proxy/auth-правки
в Rust OC не изменялись.

## Источники и границы сравнения

| Проект | Проверенная версия | Хранение |
| --- | --- | --- |
| OpenAI Codex | `67f0994b532b8740f14b5b738af7b7b2a915b1d6` | JSONL; в режиме Paginated — SQLite-проекция transcript и ссылки на префиксы других rollouts |
| Rust `0FL01/oc` | локальный `/home/qweasd123tg/Code/oc`, HEAD `feeea77fc6af538cd7e841c525a752d6fa4ba475` | SQLite как основное структурированное хранилище; часть payload остаётся JSON TEXT |
| JS OpenCode | `anomalyco/opencode`, `bf544cd419c5f17535c0d5af929f7ecd41188320` | Отдельные SQLite rows для сообщений и частей, с JSON payload |
| Pi | `earendil-works/pi`, `b79f21095de8e207654e78419a741b78a4e6a449` | JSONL записей с `id`/`parentId`, summary и ссылкой на сохранённый хвост |
| Proteus Core | `25913957829edbc45cee610aa01c8662a7b6a1c3` | Канонический JSONL, content-addressed blobs, производный SQLite read model |

Rust OC — самостоятельный Rust-проект, а не локальная сборка JS OpenCode.
Для Codex изучен свежий upstream, а не только baseline от 1 сентября,
закреплённый в старых исследованиях Proteus. Выводы ниже относятся к указанным
версиям и конкретным путям исполнения, а не ко всем режимам каждого продукта.

## 1. SQLite должна догонять журнал, а не только экономить запись строк

В Paginated Codex SQLite хранит `next_rollout_byte_offset` и
`next_rollout_ordinal`. Следующее чтение начинается с этой позиции. Строки
проекции и новый checkpoint публикуются одной транзакцией после writer flush
barrier JSONL. Пересборка идёт пакетами: порог 1 МиБ исходных данных либо
256 шагов, очередь на два пакета.

Источники: [порядок записи][cx-write], [чтение от checkpoint][cx-materialize],
[транзакция][cx-transaction], [очередь][cx-pipeline].

У Proteus [project_read_model][pr-project] сначала выполняет строгую загрузку
и строит полный read model. [SQLite saver][pr-save] уже не перезаписывает
неизменённые строки, но получает полностью построенную модель. [Transcript renderer][pr-render]
делает четыре прохода по выбранным типам записей после полной строгой проверки.
Поэтому наличие SQLite ещё не устраняет повторное вычисление представления.

**Переносимый механизм:** транзакционная позиция обработанного журнала и
индексы зависимостей для обновления затронутых строк. Одна поздняя запись
может менять видимость старого ответа, дату или результат tool; простого
«добавить строки из хвоста» недостаточно. При отсутствии подходящего состояния
или смене источника нужна полная пересборка текущей проекции.

Ограниченный пакет не равен строгому лимиту heap: одна большая запись может
превысить порог, а producer, очередь и consumer одновременно держат свои данные.
Нужны отдельные правила размера payload и суммарного количества байтов в работе.

## 2. Сохранять факты один раз, повторный контекст задавать ссылками

Rust OC записывает компактный `RequestIdentity`: model/provenance,
`input_start`, лимиты и fingerprints контекста/tools. Новые input/tool results
хранятся отдельно в `TurnLog.input`. Источники:
[RequestIdentity][oc-receipt], [TurnLog][oc-turn-log],
[запись перед generation][oc-generation].

У OpenCode summary содержит `metadata.keepID` — начало оставленного хвоста.
У Pi compaction содержит `firstKeptEntryId`; branch строится по parent links.
Это позволяет описать сохранённый контекст без очередной сплющенной копии
всех прежних сообщений. Источники: [OpenCode history][js-history],
[Pi context][pi-context], [Pi append/compaction][pi-append].

**Переносимый механизм:** неизменяемые message/part/tool bodies и точный
манифест их порядка, идентичностей и преобразований в request/hook/checkpoint.
Один fingerprint не восстанавливает запрос. Proteus должен сохранять версии
проекции, config и tool schema, а также все данные, необходимые для точного
replay. Ссылка не должна превращаться в повторное исполнение hook или provider
при чтении журнала. Отсутствующий или изменённый обязательный payload — ошибка.

## 3. Рабочее состояние и архив имеют разные размеры и правила чтения

Rust OC закрывает префикс текущего turn в неизменяемые `turn_raw_segments`
и заменяет рабочий `turns.result` в одной SQLite-транзакции. `RawPrefix`
содержит ordinal и фиксированные счётчики, а не список всех старых сегментов.
RAW защищён от UPDATE/обычного DELETE триггерами. Источники:
[атомарный seal][oc-seal], [защита RAW][oc-raw-guard],
[фиксированный RawPrefix][oc-prefix].

Paginated Codex читает модельный контекст с конца блоками по 64 КиБ и
останавливается на полном `CompactedItem` с `replacement_history` и
`window_number`. Resume metadata включает прежние settings, dynamic tools и
turn identity. Источники: [граница остановки][cx-context],
[состав checkpoint][cx-checkpoint], [resume metadata][cx-resume].

**Переносимый механизм:** явный checkpoint рабочего состояния плюс архив
с адресным чтением. Для Proteus checkpoint должен покрывать также состояние
строгого replay: незавершённые exchanges, ownership, usage и continuation
workflow/hooks. History checkpoint сам по себе этого не гарантирует.

Политики целостности различаются. Rust OC при обычном resume проверяет
последний RAW descriptor, исторические payload — при адресном чтении.
Codex и Pi могут пропускать повреждённые записи. Proteus не должен наследовать
это поведение: производная SQLite-проекция не становится authority исполнения.
Сохранённый offset или хеш манифеста сами по себе не доказывают неизменность
всех старых внешних blobs. Без отдельного обоснования доверия холодное строгое
восстановление сохраняет полную проверку; безусловное O(хвост) не обещается.

## 4. Ограничивать данные до загрузки, отдельно от UI preview

Rust OC отбирает активный контекст SQL-запросом и проверяет размер до передачи
payload в Rust; превышение активного лимита 16 МиБ даёт явную ошибку. UI
использует страницы по `seq`, до 100 сообщений. Эти лимиты различны:
100 строк не ограничивают их суммарные байты. Источники:
[UI page][oc-ui], [active SQL][oc-active], [runtime limit][oc-context-limit].

У Pi tool output может потоково уходить в файл; preview читает head/tail
позиционно, без загрузки середины. У Rust OC file-backed output имеет quota,
лимит ресурса и lease для защиты открытого файла от GC. Источники:
[Pi accumulator][pi-output], [OC output limits][oc-output],
[OC output writer][oc-output-writer], [OC GC][oc-output-gc].

**Переносимый механизм:** размер и admission проверяются до крупных
allocation/clone; preview и обязательный replay payload разделены явно.
TTL нельзя применять к единственной канонической копии результата.

Не все реализации обеспечивают backpressure всей цепи: Pi не ожидает `drain`
после `WriteStream.write`, очереди live PubSub OpenCode не ограничены, а
Codex writer queue ограничена числом команд, не байтами. Наличие streaming API
или очереди само по себе не гарантирует верхнюю границу RAM.

## 5. Два менее очевидных механизма

- **Fork без копии истории.** Codex фиксирует ссылку на rollout и конец
  родительского префикса по ordinal/byte offset. Родителя нельзя удалить,
  пока существуют ссылки. Для Proteus это направление для веток сессий;
  нужны правила владения, GC, самостоятельного экспорта и проверки lineage.
  Источники: [lineage][cx-lineage], [fork boundary][cx-fork],
  [защита удаления][cx-delete].
- **Revision вместо копии snapshot metadata.** Rust OC закрывает интервалы
  действия версий, checkpoint хранит `revision:N`, metadata переиспользуются
  по `UNIQUE(kind,payload)`. Это кандидат против повторяющихся структур
  checkpoints, с ценой schema/trigger complexity и дискового version history.
  Источник: [OC revisions/checkpoints][oc-revision].

## Что сравнение не доказывает

Codex продолжает использовать JSONL, а SQLite OC/OpenCode содержит JSON.
Замена codec не устраняет полные повторяющиеся snapshots и лишние проходы.
Бинарный формат или zstd могут уменьшить байты/CPU отдельных операций, но их
эффект нужно измерять отдельно после устранения повторной работы.

У Pi все разобранные journal entries остаются в `fileEntries`/`byId`:
compaction модельного контекста не означает ограниченный heap сессии.
У OpenCode выбранный model tail материализуется через `.all()`.
У Codex legacy loader и compatibility `includeTurns=true` могут собирать
полную историю. Нельзя объявить любой из этих агентов целиком более экономным
по результату чтения удачных участков кода.

Transcript projection Codex перестраивается из rollout, но не вся его SQLite
является disposable cache: отдельные пользовательские metadata updates
авторитетны в БД. Writer flush также не равен `fsync`. Эти различия не дают
оснований ослаблять гарантии хранения Proteus.

Источники ограничений: [Pi reader][pi-reader], [Pi retained entries][pi-retained],
[Codex permissive loader][cx-loader], [Codex compatibility hydration][cx-compat],
[Codex metadata authority][cx-authority], [Codex file flush][cx-flush].

## Рекомендация для Proteus и критерии будущей реализации

Первый ограниченный шаг — инкрементальное вычисление read projection после
новых записей, с сохранением строгой проверки и полной пересборки при
инвалидации. Это напрямую относится к повторным проходам текущего Core.
Нормализация повторяемых request/hook payload и полный operational checkpoint
— отдельные изменения формата; их не стоит смешивать с первым шагом.

Корректность будущего изменения проверяется сравнением с полным строгим
projector на late commit, replacement/compaction, timestamps, tool lifecycle,
cancel/restart и повреждённых journal/blobs. Отдельно проверяются rollback
между append и SQLite commit, смена/усечение источника и неизменность replay.
Runtime-выигрыш проверяется существующим
[history-scale](../performance/history-scale/README.md): 5k/50k сообщений,
RSS/PSS/peak, CPU и wall time; отдельно тёплое обновление, холодная пересборка,
повторный старт и один большой payload. До этих замеров ускорение не заявляется.

[pr-project]: ../../crates/proteus-core/src/core/session_store/read_model.rs
[pr-save]: ../../crates/proteus-core/src/core/session_store/read_model/sqlite/save.rs
[pr-render]: ../../crates/proteus-core/src/app_server/transcript/journal/source.rs
[cx-write]: https://github.com/openai/codex/blob/67f0994b532b8740f14b5b738af7b7b2a915b1d6/codex-rs/thread-store/src/local/live_writer.rs#L419-L469
[cx-materialize]: https://github.com/openai/codex/blob/67f0994b532b8740f14b5b738af7b7b2a915b1d6/codex-rs/thread-store/src/local/thread_history_materialization.rs#L28-L139
[cx-transaction]: https://github.com/openai/codex/blob/67f0994b532b8740f14b5b738af7b7b2a915b1d6/codex-rs/thread-store/src/local/thread_history.rs#L104-L239
[cx-pipeline]: https://github.com/openai/codex/blob/67f0994b532b8740f14b5b738af7b7b2a915b1d6/codex-rs/thread-store/src/local/thread_history_materialization_pipeline.rs#L14-L98
[cx-context]: https://github.com/openai/codex/blob/67f0994b532b8740f14b5b738af7b7b2a915b1d6/codex-rs/rollout/src/model_context.rs#L18-L57
[cx-checkpoint]: https://github.com/openai/codex/blob/67f0994b532b8740f14b5b738af7b7b2a915b1d6/codex-rs/history/src/lib.rs#L295-L315
[cx-resume]: https://github.com/openai/codex/blob/67f0994b532b8740f14b5b738af7b7b2a915b1d6/codex-rs/history/src/compaction_resume_metadata.rs#L13-L46
[cx-lineage]: https://github.com/openai/codex/blob/67f0994b532b8740f14b5b738af7b7b2a915b1d6/codex-rs/thread-store/src/local/rollout_lineage.rs#L18-L36
[cx-fork]: https://github.com/openai/codex/blob/67f0994b532b8740f14b5b738af7b7b2a915b1d6/codex-rs/thread-store/src/local/paginated_fork.rs#L91-L117
[cx-delete]: https://github.com/openai/codex/blob/67f0994b532b8740f14b5b738af7b7b2a915b1d6/codex-rs/thread-store/src/local/delete_thread.rs#L182-L227
[cx-loader]: https://github.com/openai/codex/blob/67f0994b532b8740f14b5b738af7b7b2a915b1d6/codex-rs/rollout/src/recorder.rs#L1135-L1200
[cx-compat]: https://github.com/openai/codex/blob/67f0994b532b8740f14b5b738af7b7b2a915b1d6/codex-rs/app-server/src/request_processors/thread_processor.rs#L3548-L3616
[cx-authority]: https://github.com/openai/codex/blob/67f0994b532b8740f14b5b738af7b7b2a915b1d6/codex-rs/thread-store/src/local/update_thread_metadata.rs#L428-L443
[cx-flush]: https://github.com/openai/codex/blob/67f0994b532b8740f14b5b738af7b7b2a915b1d6/codex-rs/rollout/src/recorder.rs#L2156-L2162
[js-history]: https://github.com/anomalyco/opencode/blob/bf544cd419c5f17535c0d5af929f7ecd41188320/packages/core/src/session/history.ts#L183-L264
[pi-context]: https://github.com/earendil-works/pi/blob/b79f21095de8e207654e78419a741b78a4e6a449/packages/coding-agent/src/core/session-manager.ts#L470-L511
[pi-append]: https://github.com/earendil-works/pi/blob/b79f21095de8e207654e78419a741b78a4e6a449/packages/coding-agent/src/core/session-manager.ts#L1214-L1295
[pi-output]: https://github.com/earendil-works/pi/blob/b79f21095de8e207654e78419a741b78a4e6a449/packages/coding-agent/src/core/tools/output-accumulator.ts#L29-L191
[pi-reader]: https://github.com/earendil-works/pi/blob/b79f21095de8e207654e78419a741b78a4e6a449/packages/coding-agent/src/core/session-manager.ts#L604-L669
[pi-retained]: https://github.com/earendil-works/pi/blob/b79f21095de8e207654e78419a741b78a4e6a449/packages/coding-agent/src/core/session-manager.ts#L987-L998
[oc-receipt]: https://github.com/0FL01/oc/blob/feeea77fc6af538cd7e841c525a752d6fa4ba475/crates/oc-core/src/queries.rs#L372-L400
[oc-turn-log]: https://github.com/0FL01/oc/blob/feeea77fc6af538cd7e841c525a752d6fa4ba475/crates/oc-adapters/src/tools.rs#L1462
[oc-generation]: https://github.com/0FL01/oc/blob/feeea77fc6af538cd7e841c525a752d6fa4ba475/crates/oc-adapters/src/runtime/turn.rs#L2632
[oc-seal]: https://github.com/0FL01/oc/blob/feeea77fc6af538cd7e841c525a752d6fa4ba475/crates/oc-adapters/src/storage_turn_history.rs#L318-L398
[oc-raw-guard]: https://github.com/0FL01/oc/blob/feeea77fc6af538cd7e841c525a752d6fa4ba475/crates/oc-adapters/src/storage_turn_history.rs#L206
[oc-prefix]: https://github.com/0FL01/oc/blob/feeea77fc6af538cd7e841c525a752d6fa4ba475/crates/oc-adapters/src/tools/turn_history.rs#L7
[oc-ui]: https://github.com/0FL01/oc/blob/feeea77fc6af538cd7e841c525a752d6fa4ba475/crates/oc-adapters/src/storage.rs#L1342
[oc-active]: https://github.com/0FL01/oc/blob/feeea77fc6af538cd7e841c525a752d6fa4ba475/crates/oc-adapters/src/storage.rs#L1811
[oc-context-limit]: https://github.com/0FL01/oc/blob/feeea77fc6af538cd7e841c525a752d6fa4ba475/crates/oc-adapters/src/runtime/context.rs#L1078
[oc-output]: https://github.com/0FL01/oc/blob/feeea77fc6af538cd7e841c525a752d6fa4ba475/crates/oc-adapters/src/storage_tool_output.rs#L9
[oc-output-writer]: https://github.com/0FL01/oc/blob/feeea77fc6af538cd7e841c525a752d6fa4ba475/crates/oc-adapters/src/storage_tool_output.rs#L678
[oc-output-gc]: https://github.com/0FL01/oc/blob/feeea77fc6af538cd7e841c525a752d6fa4ba475/crates/oc-adapters/src/storage_tool_output.rs#L622
[oc-revision]: https://github.com/0FL01/oc/blob/feeea77fc6af538cd7e841c525a752d6fa4ba475/crates/oc-adapters/src/storage_conversation.rs#L100-L246

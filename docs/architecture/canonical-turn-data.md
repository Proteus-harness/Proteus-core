# Canonical Turn Data

Текущий формат — journal schema v21, config snapshot v8 и session metadata v4. Resume history,
transcript, eval, prompt replay и workflow replay читают canonical journal.

Canonical context parts сохраняют обязательный `ContextChunk.render_mode`:
`source_annotated` или `verbatim`. Model requests сохраняют обязательный
`ToolSpec.supports_parallel_tool_calls` независимо от `safety`. При replay
эти поля переносятся без переинтерпретации metadata. Reader принимает только
текущую journal schema; миграций и автоматического выбора старой формы нет.

Config snapshot сохраняет для каждого registered tool его `source`, `spec` и
обязательный `model_visible`. Workflow replay восстанавливает эту видимость:
user-only команды не попадают в model context и не требуют запуска исходного
tool component. Старый snapshot без этого поля не читается.

## Решение

Storage/replay/eval контур строится вокруг одного
append-only **session journal**. Его versioned records становятся
канонической записью принятых user messages, model exchanges, tool lifecycle,
изменений conversation history и settlement turn-а.

Resume history, transcript, replay input и eval rows являются
проекциями этого journal, а не независимыми источниками правды. Event log остаётся
телеметрией: он может быть отфильтрован, усечён или отключён и поэтому не
используется для восстановления execution facts.

Это решение не означает «сохранять provider wire». Канонической остаётся
provider-neutral модель Proteus; OpenAI/Anthropic payload живёт только внутри
adapter-а и может сохраняться отдельно как opt-in diagnostic artifact.

## Границы Канонической Модели

Канонический semantic payload остаётся в `proteus-contracts`:

- `CanonicalMessage` и его parts;
- `CanonicalModelRequest` / `CanonicalModelResponse`;
- `ToolCall` / `ToolResult`;
- `AgentTask`, ids, usage и compaction report.

Journal DTO и storage envelope принадлежат `proteus-contracts`. Чистые правила
validation, redaction и проекций находятся в `proteus-session-semantics`.
Выбранный `session_storage/v2` владеет физическим форматом, writer lease,
транзакцией и восстановлением. Core назначает record identity, ожидаемую
revision и порядок записи в своей lifecycle boundary, затем проверяет durable
ACK. Workflow, hook и provider adapter возвращают contract DTO через свои
обычные callbacks.

`CanonicalModelResponse` содержит непустой ordered `messages`, а не один
синтетический assistant message. `CanonicalMessage.phase` опционально
различает `commentary` и `final_answer`; journal и history projection сохраняют
item boundaries и phase без provider-specific parsing. Singular legacy shape
не читается.

BoundModel записывает полный окончательный request после shaping и hooks до
вызова provider. Для stream validation он сохраняет только request message,
part и call identities и owned `ModelResponseValidationContext`, а не вторую
копию historical payload. Context создаётся заново для каждого prepared request
и используется одновременно для MessageCompleted и terminal response; tools
между вызовами не кэшируются. Owned context хранит только имена и форму вызова
в исходном порядке объявлений, без копий описаний и JSON-схем. Public
request-based validator использует тот же
context заимствованно, без копирования tools. При расширении request-specific
правил меняются поля context и обе формы его construction, не фиктивный request.
Provider DTO, полный journal request и failure reconciliation не меняются.

### Parts

`ContentPart` обёрнут в явный `CanonicalPart` со стабильным
`part_id`, provenance и scope. Структурная семантика не угадывается по
`message.name` или свободному `metadata`.

Минимальные измерения:

- `provenance`: user, model, tool, context builder, compactor или runtime;
- `scope`: `conversation`, `request` или `trace`;
- typed payload: текущие `Text`, `Image`, `Context`, `FileRef`, `ToolCall`,
  `ToolResult`, `Patch`, `ReasoningSummary`, `Reasoning`,
  `HostedToolActivity`, `Citation`.

`conversation` участвует в durable history. `request` живёт в конкретном
model request (например свежий `ContentPart::Context`) и не попадает в resume
history. `trace` нужен для диагностики, но не отправляется модели. Renderability
является свойством UI projection, а не четвёртым storage scope.

Reasoning signatures, tool call ids и исходные provider arguments, уже
представленные canonical DTO, сохраняются без текстового flattening. Raw
chain-of-thought не становится обязательной частью journal.

`Image` хранит provider-neutral `ImageRef` на байты, сохранённые host-ом в
session attachment store. Ссылка входит в conversation history; Base64
provider payload собирает model adapter при вызове. Cold resume использует
сохранённые байты, а workflow replay — записанный model outcome без повторной
отправки изображения. Вход и ограничения описаны в [images.md](../guides/images.md).

`HostedToolActivity` и `Citation` являются canonical response parts для
provider-side execution и сохраняются в journal/transcript/eval projections,
но не превращаются в локальную пару
`ToolCall`/`ToolResult` и не дают replay права повторить hosted side effect.
`CanonicalPart` явно закрепляет их provenance/scope; угадывать hosted execution
по provider metadata или тексту ответа нельзя.

## Формат Journal

Один canonical record содержит строгий envelope. JSONL backend сохраняет его
одной строкой, SQLite — транзакционной записью. Общие поля сохраняют одинаковую
семантику в обеих реализациях.

```text
schema_version
record_id
session_seq
timestamp_ms
session_id
execution_id? # обязателен для TurnOpened, context, model и tool facts
thread_id?    # conversational attribution
turn_id?      # conversational attribution
kind
payload
```

`session_seq` монотонен внутри session и задаёт единственный порядок между
root/child threads. `record_id` идентифицирует record и делает дубликат
обнаруживаемой corruption; отдельного публичного retry API writer не даёт.
Порядок event log не переиспользуется: telemetry fan-out и canonical commit
имеют разные гарантии.

Минимальный набор `kind`:

- `turn_opened` — task, base history revision и runtime/config snapshot;
- `history_mutated` — append принятых user/steering или workflow messages либо
  replace после compaction; содержит previous/new revision и сами canonical
  messages;
- `context_request_recorded` — точный `AgentTask` и host-generated `invocation_id`
  до вызова execution-bound builder;
- `context_response_recorded` — полный `ContextBundle` либо ошибка с отдельными
  `message` / `causes`, связанная по `invocation_id`;
- `model_request_recorded` — полный request после `RequestShaper`, до adapter
  call, с `exchange_id` и обязательным typed `origin` (`direct` или `auxiliary`);
- `model_message_recorded` — принятый completed item с `exchange_id`, до доставки workflow;
- `model_response_recorded` — terminal canonical response или canonical error,
  связанный с `exchange_id`;
- `tool_call_recorded` — call и policy/approval resolution до потенциального
  side effect;
- `tool_result_recorded` — post-orchestrator canonical result, связанный по
  `call_id`;
- `turn_settled` — success/error/cancel/timeout и итоговый `AgentOutput`, если
  он существует.

Tool call пишется до invocation, result — после. Поэтому после crash незакрытая
пара означает «результат неизвестен» и никогда не разрешает replay
автоматически повторить mutating tool. Аналогично request без response —
оборванный model exchange, а не пустой ответ. Незакрытый context build также
остаётся незавершённым фактом; projection возвращает его ID в
`interrupted_context_builds`, не создавая пустой bundle или результат повторного вызова.

Context build принадлежит workflow execution; attribution задаёт bound recorder
через journal header. Прямой `ctx.context.build` и `host.context.build` получают
одну обёртку со scope/attribution validation, cancellation и context timeout.
Detached workflow не требует модели или conversation identity. Outcome
подтверждается journal до доставки вызывающему workflow; чужой owner, orphan
response и повтор request/response ID отклоняются общей validation.

`origin` назначает Core на границе host callback: прямой вызов модели workflow
получает `direct`, callback hook/tool — `auxiliary`. Typed OperationOwner
(Workflow, Hook с invocation_id, Tool с call_id) задаёт родительскую связь. Небольшой invocation
scope отделён от `ExecutionScope`; происхождение не выводится из `module_id`
или provider metadata и не меняет authority. Поле принадлежит journal envelope,
а не `CanonicalModelRequest`: provider request и process/wire contracts не меняются.

`ModelMessageRecorded` не меняет conversation history автоматически. Workflow
выбирает completed item checkpoint-ом; terminal обязан сохранять его ids,
содержимое и порядок. Replay получает записанные completed items до terminal.
Checkpoint position сравнивается по начатым direct requests и tool facts:
при независимом model IO момент terminal относительно tool callback может
различаться. Для calls, явно выбранных текущими result bindings, момент
request/result относительно checkpoint также недетерминирован и исключён из
счётчиков позиции. Точные bindings, snapshots, остальные tool boundaries и
порядок результатов в следующем request/history по-прежнему сравниваются.

Во время stream checkpoint может расширить model prefix с теми же result
bindings. Уже записанные результаты переносятся за новый prefix в порядке
bindings; изменение identities/operation или потеря результата отклоняются.
После drain workflow включает результаты в history и снимает bindings.

Generic `ExecutionRecorder` принимает модельный `ModelFailure` целиком;
session recorder записывает canonical error с обязательным `failure`:
`ModelFailure { kind, message, completed_messages }`.
Если поток прервался после `MessageCompleted`, завершённые assistant messages
записываются в error outcome с исходными ids/parts/phases. Пустой массив означает
отсутствие такого progress; текстовые дельты не восстанавливаются в messages.
Завершённые function/freeform calls сохраняются как `ToolCall` parts;
незавершённые аргументы и tool results сюда не входят. Повторные call ids и
несоответствие объявленной tool surface отклоняются до принятия progress.
Запись error сама по себе не изменяет history: workflow выбирает progress через
checkpoint или `WorkflowFailure.history`. `coding.codex_loop` сохраняет
завершённые сообщения прямого model call и исполняет завершённые calls через
общую tool boundary до retry либо возврата ошибки. Workflow replay получает
progress вместе с записанной ошибкой и использует записанные tool outcomes.
Этот путь требует terminal Error; crash или внешняя отмена до его записи
не получают неявной history mutation из live events.

Initial user prompt записывается `history_mutated/append` до запуска workflow,
сохраняя текущую failure semantics. Steering после доставки становится
обычным user message с тем же `MessageId` и target `TurnId`; сам
process-resident queued receipt не выдаётся за durable turn fact.

Если workflow завершился `WorkflowFailure` с явным history update, Core
валидирует его и записывает ещё не подтверждённый suffix до `turn_settled(error)`.
Сохранённые assistant messages и tool results доступны следующему turn и cold
resume, хотя итог предыдущего turn остаётся ошибкой. Без явного history checkpoint
model/tool records сами по себе не добавляют сообщения в active history.
Checkpoint и выбранные им tool results переживают потерю worker-а без terminal
update; стек и локальное состояние workflow не восстанавливаются.

`model_response_recorded/error.failure.message` содержит текст ошибки, переданной
вызывающему workflow, без дополнительных префиксов writer-а. Это позволяет
workflow replay сравнивать terminal error без удаления диагностик по эвристике.
`failure.kind` сохраняется вместе с сообщениями; replay передаёт тот же
`ModelFailure`, поэтому workflow может выбрать ту же ветку по типу ошибки.
Текст ошибки не используется для восстановления её класса. Старый error payload
с отдельными `message`/`completed_messages` не принимается.

## History И Compaction

Conversation history — fold явных history mutations и объявленных tool results:

- `append` добавляет canonical messages;
- `replace` указывает входную revision, полный replacement и
  `HistoryCompactionReport`;
- `checkpoint` сохраняет подтверждённый workflow snapshot и ordered
  `tool_results` bindings: исходный model call остаётся в history, а обязательный
  `execution_call` задаёт точную операцию с тем же call id. Tool lifecycle
  записывает эту операцию; replay проверяет и history, и execution binding.
  Формат binding и validation описаны в
  [process-module-architecture.md](process-module-architecture.md);
- root `tool_result_recorded`, выбранный активным checkpoint, вставляет canonical
  tool message с заранее выделенными ids в порядок bindings и увеличивает history
  revision. Результат и его участие в history — один durable record; отдельного
  acknowledgement от workflow не требуется;
- mismatch revision является corruption/concurrency error, а не поводом
  «починить» порядок эвристикой.

Compaction меняет активную проекцию, но не удаляет старые journal records.
Поэтому resume получает короткую history, а replay/eval всё ещё видят
pre-compaction exchanges и точную lineage. Отдельные
`messages.pre-compaction.N.jsonl` после перехода больше не нужны как источник
данных.

Resume восстанавливает history mutations и выбранные tool results из journal.
Перестраиваемые кеши привязаны к session, revision и hash committed prefix.
Потеря кеша запускает восстановление из canonical records.

## Большие Payload

Storage передаёт Core hydrated canonical DTO и проверяет размер и SHA-256
content-addressed blobs. Отсутствующее или повреждённое значение приводит к
ошибке чтения. `PutBlob` и `ReadBlobByHash` обслуживают session attachments,
включая изображения для cold resume.

JSONL backend хранит большие payload в session-local `blobs/`. Его journal
value поддерживает inline JSON и reference `{sha256, bytes, relative_path}`.
Пути ограничены каталогом сессии. SQLite backend хранит journal в своей базе.
Физический threshold и кодек принадлежат реализации storage.

## Запись И Recovery

- Core назначает `operation_id`, timestamp, attribution и ожидаемые
  `session_seq` / `history_revision` до отправки `Commit`. Модуль сохраняет
  `operation_id` как `record_id` атомарно с фактом и возвращает ACK после
  durable commit. Core проверяет identity и revision ответа.
- Storage выдаёт writer lease с token и generation. OS-lease исключает второго
  writer process. Core сериализует принятые операции одного session handle.
  Последний release освобождает ownership; hibernate освобождает декодированное
  состояние при сохранённой lease.
- Потеря транспорта после отправки записи оставляет её исход неизвестным.
  `CommitStatus` устанавливает результат по исходному `operation_id`.
  Идентичный повтор этой операции возвращает прежний ACK. Другое содержимое
  с тем же id отклоняется.
- Read-only snapshot закрепляет точный committed prefix по revision и hash.
  Все страницы, история, usage и replay читаются через storage API. Дописанный
  хвост сохраняет прежний prefix; изменённый или усечённый prefix вызывает
  ошибку. Read-only открытие сохраняет journal.
- Восстановление writer проверяет canonical факты и blobs. JSONL backend
  читает записи потоком, восстанавливает незавершённую последнюю строку и
  отклоняет повреждённую полную строку. Steady-state append использует уже
  подтверждённое состояние. SQLite использует транзакции WAL с
  `synchronous=FULL`.
- Принятая Core transaction продолжает commit/reconciliation при отмене
  ожидающего caller. Следующая запись ждёт её завершения. Публикация
  history checkpoint и влияющих на resume событий следует после ACK.
- `session_seq` увеличивается для каждого journal record. `history_revision`
  увеличивается для успешно принятого history mutation или tool result с
  активным history binding.
- Secrets/redaction применяются до durable записи с сохранением schema fields.
  Resume, transcript и replay используют фактически подтверждённый record.

Точная процессная граница и её authority описаны в
[process-module-architecture.md](process-module-architecture.md#session-storage).

Key-based redaction не входит в schema-definition поля canonical request и
config snapshot: `ToolSpec.input_schema`, function `output_schema` и
`ResponseFormat::JsonSchema.schema` сохраняются без изменений. Те же
sensitive keys в value-bearing metadata, client metadata и tool arguments
по-прежнему заменяются до сериализации; произвольный metadata-путь не может
объявить себя schema boundary совпадением имени.

## Проекции И Replay

Из одного journal строятся или могут быть построены:

- resume history — fold history revisions (**реализовано**);
- история сообщений приложения — render conversation parts и terminal tool cards
  (**реализовано**; live delta-tail остаётся process-resident);
- trace — lifecycle view с ids и длительностями, дополненная event log для
  live deltas;
- eval rows — task, exact shaped requests/responses, tool decisions/results и
  outcome без парсинга UI-текста (**реализовано**);
- prompt replay — повтор provider call по сохранённому request
  (**реализовано** командой `proteus replay prompt`);
- workflow replay — подстановка записанных model/tool результатов без внешних
  side effects (**реализовано** командой `proteus replay workflow`).

«Живой rerun tools» — отдельный опасный режим, не replay по умолчанию.
Provider wire replay также не является canonical: adapter снова формирует wire
из сохранённого canonical request.

### Prompt Replay v0

`proteus --config <profile> replay prompt <session-dir-or-journal-path>` читает
journal через общий `SessionStore`/`JournalProjection` reader и выбирает
завершённую пару `model_request_recorded` + `model_response_recorded`:

- переданный `--exchange-id` ищется строго;
- без id автоматически выбирается только единственный завершённый exchange;
- несколько exchanges требуют явного id и выводят список доступных ids;
- неизвестный или незавершённый exchange завершается ошибкой без догадок.

Сохранённый `CanonicalModelRequest` уже находится после `RequestShaper`, поэтому
replay передаёт его напрямую model adapter-у. Context building, compaction,
selection hooks, повторный shaping, workflow и `ToolOrchestrator` не запускаются.
Так как `ModelRef` является частью exact request, `recorded_model` и
`replay_model` в v0 совпадают; выбранный transport фиксируется отдельно в
`replay_adapter`.

Local tool calls из нового ответа только подсчитываются и перечисляются в
отчёте. Provider-hosted tools в request по умолчанию блокируют replay, потому
что provider может выполнить внешний side effect внутри model call. Явный
`--allow-hosted-tools` разрешает отправить исходный request целиком, без
фильтрации или переписывания.

Prompt replay v0 не дописывает records в исходный journal и не создаёт durable
replay run. Human/`--json` отчёт schema v2 содержит обязательный
`ExecutionId`, optional session/thread/turn source ids, model/adapter,
recorded/replay outcome и usage, text equality, local/hosted/citation counts и
длительность adapter call. Различие текста является результатом
недетерминированной генерации, а не ошибкой команды.

### Workflow Replay v0

Replay сохраняет записанный `ExecutionId`: он доступен workflow/v22 и может
участвовать в deterministic ids и terminal metadata.

`proteus --config <profile> replay workflow
<session-dir-or-journal-path> [--turn-id <id>] [--json]` повторяет один
сохранённый root turn через записанный Workflow и Policy. Команда берёт module
ids, model/reasoning, tool specs, default permission mode и approval availability из
`turn_opened.config_snapshot`; текущий profile нужен для доступных module
factories, их settings и instruction blocks. Если journal содержит несколько
turns, `--turn-id` обязателен. Неизвестный id, child turn, незавершённый
model/tool record, overlap turns или runtime-owned `Canceled`/`Timeout`
отклоняется без эвристики.

До построения последовательности replay проверяет завершённость всех model/tool
операций и owner graph выбранного turn. Workflow-owned Direct exchanges остаются
в последовательности workflow. Hook/Tool-owned Auxiliary exchanges потребляются
с записанным родительским subtree ровно один раз, без повторного исполнения.
BeforeToolSelection trace проверяет полный immutable input/candidates и
возвращает точный отобранный набор; BeforeContext trace возвращает результат
подготовки, включая failure или unchanged. Оба не требуют следующего Direct call. Подробности — в
[hook-tool-operations.md](hook-tool-operations.md).

Replay runtime строит выбранные Workflow и Policy, но не вызывает real provider
adapters, subagents или настоящие tools. Model responses и tool results последовательно берутся из
`model_response_recorded`/`tool_result_recorded`; context возвращается по
точным context request/response records, отбор tools и preparation — из typed hook traces. Approval
проходит обычный `ApprovalPolicy -> ToolOrchestrator` path, но ответ transport-а
и результат tool invocation уже записаны в journal. Поэтому mutating tool и
provider-hosted side effect повторно не выполняются.

Runner проверяет каждый context build input и consumption записанного outcome.
Для concurrent builds с разными входами выбирается первый ещё не потреблённый
record с точным `AgentTask`; одинаковые входы потребляются в порядке request
records. Replay не воспроизводит timing завершения read-only builds.
Незаписанный, изменённый или пропущенный вызов означает divergence, даже если
workflow игнорирует ошибку и возвращает прежний итог. Model request не служит
источником context bundle; compaction не требует placeholder chunks.

Runner сравнивает каждый post-shaping model request, tool request/approval/
resolution/result, changed compaction report, settlement, `AgentOutput` и
итоговую persistent history. Workflow output проходит тот же core-owned
history validation, что и обычный root runtime. Для terminal `WorkflowFailure`
проверяется также явно возвращённый history update; успешный `AgentOutput`
при этом не создаётся.
Checkpoint snapshots, набор выбранных calls и положение callback относительно
model/tool boundaries также сравниваются; одинаковый final output не скрывает
потерю промежуточной durable записи. Lifecycles объявленных in-flight calls
могут пересекать checkpoint независимо от исходного момента завершения, но
replay всё равно требует их полное исполнение и правильный final drain.
Нормализация ограничена заново создаваемыми `MessageId`/`PartId`, внутренними
generated call ids, описанным выше scheduling объявленных in-flight calls,
недетерминированным `ToolResult.metadata.duration_ms` и
зависящим от него итоговым `AgentOutput.metadata.context.token_estimate`;
остальные различия остаются divergence. Доставленный steering/follow-up внутри
выбранного turn-а пока отклоняется fail-closed: v0 не эмулирует root steering
decorator. Обычный terminal `Error` воспроизводится как workflow outcome при
наличии завершённых records. Внешний момент client cancellation и runtime
timeout в journal не записан, поэтому `Canceled`/`Timeout` нельзя честно
получить повторным запуском Workflow: их durable contract проверяется через
canonical `TurnSettled` и cold `/history`.

Исходный journal читается до и после replay и должен остаться побайтово
неизменным. Durable replay run и новый storage format не создаются. Human и
`--json` report schema v1 используют `comparison.matched` и
`comparison.issues` как источник результата сравнения; `diverged` является
успешно сформированным диагностическим отчётом и сам по себе не меняет exit
status команды. Ошибки выбора fixture, построения записанных modules или самого
replay path завершают команду ошибкой.

Оба replay-режима являются повторным вычислением/сравнением по сохранённым
facts, а не crash continuation. Journal не сохраняет program counter,
suspended future, stack/local variables, live cancellation token или
process-resident steering queue. Projection может показать незавершённый Turn
и неизвестный результат side effect, но не умеет продолжить вычисление с места
остановки.

## Текущий Execution Owner

Journal разделяет durable execution owner и chat projection:

```text
ExecutionAttribution
  execution_id: ExecutionId
  agent: Option<AgentTurnAttribution>
    session_id / thread_id / turn_id
```

`TurnOpened`, model и tool facts всегда несут `ExecutionId`. Для обычного
agent Turn они также несут thread/turn attribution, и projection проверяет её
против mapping `TurnId -> ExecutionId`, созданного `TurnOpened`. Detached model
execution пишет request/response/error с `ExecutionId`, но без fake
`ThreadId`/`TurnId` и без открытого Turn. Model/tool lifecycle сопоставляется
по execution owner; owner нельзя изменить между request/response или
call/result.

Один `ExecutionId` не может переключаться между detached и agent attribution
или принадлежать двум Turns. После `TurnSettled` новые root-thread facts
отвергаются; ранее начатый child-thread exchange/call может завершиться позже,
что сохраняет semantics background agent work.

`HistoryMutated` и `TurnSettled` остаются chat/session lifecycle facts и не
несут `ExecutionId`. Поэтому cancellation/timeout по-прежнему может оставить
model exchange interrupted и завершить именно Turn. Generic
`ExecutionSettled` и durable continuation отсутствуют.

`ExecutionRecorder` остаётся generic scope-bound contract без chat IDs.
`SessionExecutionRecorder` адаптирует model facts к session journal через
immutable binding. Tool lifecycle записывает отдельный generic
`ToolExecutionRecorder`: attribution передаётся на каждом вызове, поэтому
root/child presentation threads сохраняются как optional projection, а
detached tool facts не требуют invented Turn. Generic lifecycle исполняет
`BoundTools`; agent-shaped `ToolOrchestrator` остался только wrapper-ом для
`AgentWorkflowContext`, presentation events, task/user-input и agent-control
enrichment. Это не ограничение journal schema.

## Проверка

- один turn полностью восстанавливается без event log;
- accepted user message переживает provider/workflow failure;
- незакрытый mutating tool после crash не запускается повторно;
- compaction сокращает resume history без потери исходных execution records;
- transcript, replay и eval ссылаются на одни `SessionId`/`ThreadId`/`TurnId`,
  `MessageId`, `CallId` и `exchange_id`;
- root module-swap tests не зависят от storage implementation;
- provider-specific wire types не выходят из adapters.

## Hook Facts И Фактический Tool Outcome

`HookInvoked` хранит typed input, ordered accepted responses/failures и output
цепочки. `ToolEffectRecorded` при активных hooks фиксирует actual result до
ожидания after-tool contribution, а `ToolResultRecorded` — final result. Final
может изменить текст output, сохраняя status/error/content. Без final исходный
результат остаётся известным; root settlement включает его в cold и живую history,
transcript и анализ хода используют raw fallback. Внутри незавершённого хода
это execution fact, а не завершённый workflow checkpoint.
Без session store recorder удерживает pending raw result до завершения workflow
и включает его в живую history, если final result не поступил.

Replay воспроизводит записанные hook decisions и raw/final result отдельно;
он не вызывает hook workers и не повторяет tool effects.

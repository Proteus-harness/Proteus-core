# Операции Hooks И Tools

Действующая граница: `hook/v8`, `tool/v7`, `workflow/v22`, journal schema v21.

Запрос модели явно задаёт `CanonicalModelRequest.operation`: обычный `Complete`
или нативный `Compact`. Отсутствие operation означает обычный Complete;
поддержка Compact всегда требует `ModelCapabilities.supports_compaction`.
Host проверяет capability до provider I/O и передаёт выбранное описание модели
в `BeforeContext.model_capabilities`; model-free подготовка получает `None`.
Workflow не может подменить этот host-owned descriptor.

Compact из hook использует тот же bound model callback, auxiliary attribution,
учёт usage и cancellation. UI stream Compact подавляется по typed operation,
независимо от формы metadata. В отличие от обычной auxiliary генерации, он
проходит настроенные `BeforeModel` transforms; вложенный auxiliary Complete
не запускает их повторно. Формат checkpoint принадлежит provider adapter:
Core сохраняет ordered `Opaque` без интерпретации и отображения. Подготовка
лишь предлагает replacement; durable history меняет workflow checkpoint после
успеха. Replay использует записанный descriptor и сравнивает operation, не
обращаясь к provider.

Отдельные slots `compactor` и `tool_exposure`, их selections, exports и
adapters удалены.
Алгоритм подготовки выбирается обычным hook export в `modules.hooks`.
Core организует вызов и принятие результата, но не выбирает алгоритм.

## Общие Операции

| Операция | Hooks | Tools |
|---|---|---|
| `host.model.complete` | Actionable phases, включая before/after-tool | Во время invocation |
| `host.tools.execute` | BeforeToolSelection, BeforeContext, BeforeModel, BeforeStop | Во время invocation |
| Conversation read/snapshot | Read-only snapshot в typed input, когда bound | Существующие conversation callbacks |

Lifecycle notifications не запускают дочернюю работу. В before/after-tool
вложенный tool callback запрещён, чтобы не создавать recursion hook chain.
Модель не обязательна: без выбранного model service callback даёт явную ошибку,
а детерминированный hook продолжает работать.

Все операции используют captured assembly, execution attribution, cwd,
cancellation и бюджет родителя. Authority не зависит от module id, языка или
совмещения exports в одном process. Callback задачи восстанавливают parent scope.

Model callback — auxiliary вызов с обычным shaping, validation, journal и usage.
Он не потребляет root steering и не публикует основной ответ; обычный Complete
не повторяет BeforeModel chain. Результат возвращается вызывающему модулю без автоматической
записи в conversation. Provider failure не подменяется специальным fallback.

Tool callback проходит BoundTools, registry, policy, approval, safety и полную
before/after-tool chain. Повтор той же registry tool identity в активной lineage
отклоняется независимо от call id. Дочерний результат не создаёт фиктивный
assistant message: запись в working history требует явного workflow binding.
Conversation snapshot и поиск источника конкретного tool call остаются разными
операциями; источник доступен только при реальном canonical owner.

## Отбор Tools

Workflow вызывает `host.tools.select` до подготовки контекста. Host получает
policy-visible candidates из registry и передаёт их в `BeforeToolSelection`:

- immutable `ToolSelectionInput`: request (task/cwd, query, max_tools, reason,
  phase) и исходные candidates;
- `ToolSelectionOutput`: предыдущий отобранный набор tools и metadata;
- optional read-only conversation snapshot, только если execution bound.

Начальный output содержит все candidates. Hook возвращает Continue либо
`ToolSelection { output }`. Каждый contribution может только сузить или
переупорядочить предыдущий набор; полные `ToolSpec` должны совпадать, дубликаты
и изменение любого поля отклоняются. Последующий handler не возвращает tools,
которые удалил предыдущий. Input и authority не изменяются. Итог снова
валидируется на host boundary и перед model request/tool execution.

Это host-owned workflow operation с общей hook chain, без выбираемого сервиса,
нового slot или обязательной модели/conversation. Скрытие schema уменьшает
model view, не меняет разрешение исполнения. Поддержанные coding workflows
добавляют `proteus_tool_search`, `proteus_tool_describe`, `proteus_tool_call`
для обнаружения и вызова скрытых tools; каждый реальный вызов проходит обычный
registry/policy/approval path.

Эвристика `hook/tool_selection.dynamic` сохраняет stable hot set, optional
explicit query, phase filtering и обязательные provider-hosted/collaboration
группы. Настройки принадлежат `module_config.hook."tool_selection.dynamic"`.
Это алгоритм Proteus, не upstream Codex tool search. В Codex-family profiles
этот hook не выбран. Telemetry итогового отбора попадает в request metadata
`tool_selection`.

Policy visibility зависит и от возможности transport запросить approval.
Host сохраняет фактический `approval_available` в admission/config snapshot;
replay использует этот факт вместо предположения об интерактивном transport.

Selection trace записывается даже для пустой chain, поэтому replay проверяет
точные request/candidates и воспроизводит результат или failure без следующего
model request. Записанный input сохраняет schemas скрытых, даже не вызванных
tools, которые могут понадобиться при replay discovery. Обработчики и их
дочерние effects повторно не исполняются.

## Подготовка Контекста

Workflow вызывает `host.context.prepare` после завершения model stream и tool
batch. Typed BeforeContext input содержит:

- ContextPreparationInput: messages, nullable canonical request template,
  token_estimate, window_tokens, reason;
- предыдущий ContextPreparationOutput в ordered chain;
- nullable ContextPreparationTarget, привязанный host к execution и revision.

Request template сохраняет model controls, instructions и cache для стратегий,
которым нужна модель; материал преобразования задаётся отдельным messages.
Private/detached preparation не требует conversation или модели и не создаёт
root checkpoint. BeforeModel остаётся request-only преобразованием.

Hook возвращает Continue либо ContextPreparation с output. Output содержит
replacement messages, changed, explicit user_message_replacements и typed
диагностику: summary, estimates, trigger_tokens, summary_source, skipped_reason,
metadata. Opaque metadata не меняет типизированные поля и числа сообщений.

Каждый contribution валидируется относительно предыдущего candidate и исходного
основания. Содержимое существующего message/part id неизменно; производные
сообщения получают свежие ids. Последовательные user derivations соединяются
с исходными пользователями. Промежуточные candidates не публикуются.
Проверка строит индексы identities за один проход; составной материал для
проверки chain использует ссылки на исходные сообщения без копии всей истории
или request template. Эти внутренние оптимизации не меняют wire-форму trace
и проверки неизменности messages, parts и user derivations.

В pre-turn Codex workflow передаёт старую историю без уже admitted текущего
пользователя. Текущий ввод остаётся защищённым suffix и возвращается неизменным
в полный replacement перед checkpoint. Свежий request context строится после
принятия подготовки.

## Локальная оценка opaque input

`hook/v8` добавляет `host.model.estimate_input` с `ModelInputEstimateRequest`:
выбранная `model` и уникальные canonical `Opaque` parts. Только selected bound
model может оценить свой материал; Core не знает provider format или формулу.
`model/v14` объявляет метод `estimate_input` опциональным feature
`model.input_estimate.v1`. Не поддерживается — `null`, а не нулевая оценка.
Ответ `ModelInputEstimate` содержит subset уникальных `part_id` и `u32 tokens`;
ноль допустим. Чужие/повторные ids и не-Opaque запросы отклоняются.

Это локальная advisory операция с owner, отменой и timeout текущего invocation.
Она не запускает BeforeModel, inference, ModelRequest или TokenUsage exchange.
У ToolHost и WorkflowHost этого callback нет. Replay использует уже записанный
hook proposal; дополнительного journal record или повторного sizing нет.

`ModelInputEstimate` читает/пишет typed map `{parts:[{part_id,tokens}]}` под ключом
`model/input_token_estimates` в metadata hook output и workflow request.
Применяются только matching Opaque ids; payload и message metadata неизменны.
`ContextTokenEstimateSource` сохраняет `estimated`/`observed` под
`context/token_estimate_source`. Helpers сохраняют другие metadata keys и не
заменяют произвольное scalar metadata. Wire DTO не получает обязательных полей,
старые записи остаются читаемыми.

## Принятие Истории

1. Host привязывает target/revision и запускает ordered BeforeContext chain.
2. Workflow сохраняет итоговый candidate отдельно от принятого состояния.
3. WorkflowHistoryCheckpoint передаёт preparation_target для проверки revision.
4. WorkflowCheckpointAck возвращает принятую cumulative history, полные canonical
   messages и revision; root ACK включает уже принятые steering/tool results.
5. Только после ACK workflow обновляет историю, model view и user identity.

Stale target отклоняется явно. Revision меняется при изменении accepted working
history, включая выбранный tool result и steering, но не при записи произвольной
auxiliary операции. Нет молчаливого rebase или повторного исполнения effects.
Checkpoint выполняется один раз для всей цепочки.

Rejected candidate не попадает в terminal output или WorkflowFailure.history.
Если checkpoint записан, а ответ потерян, host сохраняет принятое состояние и
не восстанавливает старую копию из failure. Отмена следующего sampling не
отменяет checkpoint. Исходные journal records сохраняются для cold history и
диагностики. Без conversation persistence ACK имеет revision=null и возвращает
локальный progress; это не полный root snapshot.

Tools сохраняют implementation-owned данные для подготовки, как отложенный DCP. Прямого
host.context.prepare callback у tools нет: запуском управляет workflow после
batch. Typed edit proposal непосредственно в ToolResult пока не реализован;
текст output не является скрытым способом изменить историю.

## Journal И Replay

Model requests имеют origin Direct/Auxiliary и typed OperationOwner:
Workflow, Hook с invocation_id или Tool с call_id. HookTrace сохраняет owner и
отдельные invocation ids шагов; requested tool record сохраняет parent owner.
Родителя назначает host, а не module metadata. Failed hook step сохраняет
nullable ModelFailure, чтобы callback и replay не теряли typed ошибку модели.

Replay проверяет завершённость и связность дочерних операций перед построением
последовательности workflow. Записанный hook/tool outcome потребляет его subtree
ровно один раз без реальных tools или provider calls. Вложенные model outcomes
не занимают место следующего workflow request. Отбор tools воспроизводится по BeforeToolSelection trace. Failed/unchanged
preparation воспроизводится по BeforeContext trace, без требования следующего direct call.
Cold resume читает accepted working history из journal.

## Материал подготовки и размер трассы

Workflow создаёт вход через `ContextPreparationInput::from_request`. Сообщения
переходят в `input.messages`, а шаблон запроса хранит параметры модели.
Хук возвращает `Continue` при отсутствии изменений. Для оценки токенов, причины
пропуска и других диагностических полей хост и модуль могут согласовать
`hook.context_preparation_diagnostics.v1`. При этой возможности ответ содержит
только диагностические поля. Полный результат нужен при замене истории.

Проверка нового ответа хука должна включать длинную историю и проходить через
сериализацию. Для изменения только оценки проверяются состав ответа и результат
применения к исходному кандидату. Для записи журнала проверяются размер
сериализованной трассы и точное восстановление исходного события и всех решений.
Эти проверки обнаруживают повторную запись истории даже с включённым уплотнением файлов.

## Стратегии

В отложенном эксперименте DCP сохраняет summary/prune state своим tool и строит request view в BeforeModel.
Он не меняет durable root history. Codex export `hook/codex_compact` проверяет
порог, при необходимости вызывает модель и предлагает replacement. Python suffix
реализует тот же hook contract без model callback. Workflow и Core не знают
имён этих стратегий. Настройки находятся в module_config.hook.<module_id>.

Wire остаётся v3. Старые config/contract/journal формы отвергаются; aliases и
совместимого compactor пути нет. Проверки этой границы описаны в
[testing.md](../development/testing.md).

## Проверенные Образцы

- Pi: [обычная custom compaction](https://github.com/earendil-works/pi/blob/96abe9a5ad45005cae34ed1c52d42f71fc88bedc/packages/coding-agent/examples/extensions/custom-compaction.ts),
  [typed boundary entries](https://github.com/earendil-works/pi/blob/96abe9a5ad45005cae34ed1c52d42f71fc88bedc/packages/coding-agent/src/core/extensions/types.ts#L958).
- OpenCode V2 dev: [plugin operations](https://github.com/anomalyco/opencode/blob/d21b0790a6c146815f28fea604e41e922f009590/packages/plugin/src/promise/plugin.ts),
  [prepared compaction result и publication](https://github.com/anomalyco/opencode/blob/d21b0790a6c146815f28fea604e41e922f009590/packages/core/src/session/compaction.ts#L392).

Это источники конкретных механизмов, не объявление совместимости Proteus с их
полным API. Process boundary, nested tool path и checkpoint rules
принадлежат Proteus.

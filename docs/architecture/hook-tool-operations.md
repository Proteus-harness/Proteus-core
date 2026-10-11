# Операции Hooks И Tools

Действующая граница: `hook/v5`, `tool/v6`, `workflow/v20`, journal schema v19.
Отдельный slot `compactor`, его selection, export и adapter удалены.
Алгоритм подготовки выбирается обычным hook export в `modules.hooks`.
Core организует вызов и принятие результата, но не выбирает алгоритм.

## Общие Операции

| Операция | Hooks | Tools |
|---|---|---|
| `host.model.complete` | Actionable phases, включая before/after-tool | Во время invocation |
| `host.tools.execute` | BeforeContext, BeforeModel, BeforeStop | Во время invocation |
| Conversation read/snapshot | Read-only snapshot в typed input, когда bound | Существующие conversation callbacks |

Lifecycle notifications не запускают дочернюю работу. В before/after-tool
вложенный tool callback запрещён, чтобы не создавать recursion hook chain.
Модель не обязательна: без выбранного model service callback даёт явную ошибку,
а детерминированный hook продолжает работать.

Все операции используют captured assembly, execution attribution, cwd,
cancellation и бюджет родителя. Authority не зависит от module id, языка или
совмещения exports в одном process. Callback задачи восстанавливают parent scope.

Model callback — auxiliary вызов с обычным shaping, validation, journal и usage.
Он не потребляет root steering, не публикует основной ответ и не повторяет
BeforeModel chain. Результат возвращается вызывающему модулю без автоматической
записи в conversation. Provider failure не подменяется специальным fallback.

Tool callback проходит BoundTools, registry, policy, approval, safety и полную
before/after-tool chain. Повтор той же registry tool identity в активной lineage
отклоняется независимо от call id. Дочерний результат не создаёт фиктивный
assistant message: запись в working history требует явного workflow binding.
Conversation snapshot и поиск источника конкретного tool call остаются разными
операциями; источник доступен только при реальном canonical owner.

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

В pre-turn Codex workflow передаёт старую историю без уже admitted текущего
пользователя. Текущий ввод остаётся защищённым suffix и возвращается неизменным
в полный replacement перед checkpoint. Свежий request context строится после
принятия подготовки.

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

Tools сохраняют implementation-owned данные для подготовки, как DCP. Прямого
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
не занимают место следующего workflow request. Failed/unchanged preparation
воспроизводится по BeforeContext trace, без требования следующего direct call.
Cold resume читает accepted working history из journal.

## Стратегии

DCP сохраняет summary/prune state своим tool и строит request view в BeforeModel.
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

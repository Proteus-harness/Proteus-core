# Текущее Состояние

Замысел — в [spec.md](spec.md), ожидаемый результат — в
[roadmap.md](roadmap.md). Здесь описана реализация.

## Что Работает

- Process-only Component Runtime v2 / wire v3: persistent multi-export
  components, concurrent invocation, callbacks, cancellation и restart.
- Slots для workflow, context/context providers, policy,
  tools, hooks и model; отбор tools выполняется hooks.
  Поиск, память и patch — process tools;
  автоматическое чтение поиска/памяти — context providers.
- AssemblyPlan, атомарный runtime snapshot и ExecutionScope.
- Вход «текст + изображения»: canonical image refs, хранилище вложений,
  process Workflow/Model, capability validation, OpenAI/Anthropic encoding,
  прикрепление и миниатюры в приложении, cold history/resume;
  [границы и использование](../guides/images.md).
- Standalone process Workflow без conversation и модели; typed top-level
  операция `execute_tool` без открытия Turn; `/remember` вызывает memory tool.
- Opt-in typed `hook/v6` contributions: явная ordered chain, model instructions/
  messages, отбор tools, подготовка working history, pre-tool block/args, post-tool output
  и bounded completion review. Hooks/tools получают execution-bound model/tool
  callbacks; выбранный preparation hook заменяет отдельный compactor slot.
- Самостоятельный DCP Node component с model-driven `compress` и hook context
  view, package-owned durable state; исходная история сохраняется. Это перенос
  механизма DCP 3.2.0 с явными adaptations, без OpenCode commands/TUI/RPC;
  [подключение](../../modules/reference/dcp/README.md).
- Общий tool safety/approval path и execution-bound model/tools/context providers.
- Canonical journal, history/resume, prompt replay и workflow replay
  в поддержанной границе.
- Workflow checkpoints и явные bindings tool results: выбранный прогресс
  сохраняется при потере процесса, результат неизвестного side effect не
  выдумывается. Используется `coding.single_loop`, `coding.codex_loop`,
  `coding.plan_execute_review` и Python example.
- AgentControl для полных local Proteus peers: lifecycle, bounded mailbox,
  messaging, follow-up и адресная отмена.
- CLI/REPL и HTTP/SSE/stdio app-server. Основной пользовательский клиент —
  нативное приложение Proteus на GPUI в `clients/native`.
- ACP v1 stdio agent для редакторов: независимые sessions, streamed responses,
  tools/approval, отмена, селекторы модели/reasoning/прав, план выполнения
  и editor stdio MCP через существующий runtime;
  [возможности и ограничения](../guides/runtime-and-events.md#acp-для-редакторов).
- Приложение на GPUI под Linux: клиент и backend в переносимой папке, запуск
  сервера для проекта, выбор папки, вкладки и области, настройки профиля по
  слотам и страницы аналитики (расход и контекст, анализ ходов, архитектура).
- Расширения клиента: значки у поля ввода и в полосах окна, панели и вкладки
  документов; внешние программы с `extension.json` без пересборки, установка
  ZIP-архивом, порядок и места значков; данные агента — через публичный API.
  [Контракт и границы](../../clients/native/README.md#расширения-без-пересборки).
- OpenAI, OpenAI-compatible, ChatGPT subscription OAuth (`openai_codex`),
  Anthropic и fake implementations в reference
  `model-pack`; Core использует общий `model/v12` process adapter.
- Doctor, inspect/topology, eval report и атомарная локальная установка.
- Управление доступностью skills и MCP через `/addons`, локальная загрузка
  Agent Plugins 1.0 (skills и stdio MCP), общий сервис расширений `agent.addons`.
  Отдельные страницы управления этими дополнениями пока не добавлены.
- App-server подхватывает внешние изменения профиля во всех открытых сессиях:
  проверенная сборка публикуется новым epoch, действующий ход сохраняет старый.

Reference modules и profiles — поставляемые примеры без особых прав.

## Что Пока Ограничено

| Граница | Ограничение |
|---|---|
| Model | Descriptor запрашивается для конкретного `ModelRef` и сохраняется в runtime snapshot; обновление возможностей требует пересборки snapshot |
| Workflow | Standalone invocation допускает пустую history, отсутствие conversation и модели; chat events и root history checkpoints требуют разговорного контекста; preparation частной истории возможна без модели и conversation |
| Исполнение вне чата | `Workflow` сохраняет `AgentTask` с текстом и `cwd`, history и `AgentOutput`; отдельный вызов через registry не даёт публичного admission и durable settlement произвольной задачи в `AgentRuntime` |
| Предметные контракты | `ModuleKind`, таблица process authority, config и adapters задают известный host набор; подключение нового семейства контрактов без согласованных изменений этих границ пока отсутствует |
| Replay | Model-free Turn поддержан. Context build воспроизводится по точным input/outcome records, включая ошибки до модели; отбор tools и preparation — по typed hook trace, включая failure и unchanged. Owner/parent tree связывает дочерние model/tool records с hook/tool invocation; алгоритмы и effects повторно не исполняются. Root steering и внешние Canceled/Timeout не эмулируются |
| Collaboration | Spawn принимает только parallel_safe роли с isolation=none; настроенный coder с worktree в эту surface не входит |
| Peer recovery | Resume зависит от живого process; durable tree, attach и reconnect отсутствуют |
| Доверие к модулям | Собственные процессные модули доверенные и работают с OS-правами владельца; принятый локальный режим, sandbox не является задачей этапа |
| Форматы | Config/API/DTO/wire/storage пока не стабилизированы |

Точные границы: [modules.md](../architecture/modules.md),
[architecture.md](../architecture/architecture.md),
[subagents.md](../architecture/subagents.md).
Это инвентарь ограничений, а не перечень обязательных следующих фич.
Задел для исполнения вне чата, ограничения и возможное развитие — в
[execution-runtime.md](../architecture/execution-runtime.md).

## Статус Примера Codex

Codex profile существует. Ordered commentary/final messages сохраняются
в canonical response/history/journal, live events и app transcript. Есть fixture и regression этого среза:
[codex-baseline.md](../development/codex-baseline.md).

Полного differential harness и сравнительного отчёта по обычным задачам
и расходу нет. Близость сборки пока подтверждена отдельными срезами.
Process conformance и module swap
подтверждают техническую заменяемость; они не доказывают удобство любых
комбинаций модулей или качество live агента.

## Разработка

Проект собирается и проверяется локально. CI отключён сознательно.
Применимые проверки описаны в [testing.md](../development/testing.md).

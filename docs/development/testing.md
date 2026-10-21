# Тестирование

Тест полезен, когда ловит конкретный дефект поведения или контракта.
Для обратимой низкорисковой правки не нужен отдельный тест, повторяющий
implementation. Сначала используйте или расширьте существующий сценарий.
Один инвариант проверяйте на нескольких слоях, только если каждый слой ловит
свой источник ошибки.

Не закрепляйте расположение кода, слова в исходниках, фразы настраиваемого
prompt или число компонентов в примере. Строгую схему проверяйте неизвестным
полем текущего формата; отдельные тесты для каждого удалённого имени не нужны.
Разбор specs reference tools уже проверяет
`conformance::aggregate_tool_module_lists_and_invokes_real_tools` через модуль.
Отдельный spec-тест оправдан собственной semantics, например `ToolSafety`.

## Стандарт Изменения

Для существенной правки определите проблему, ожидаемый результат и затронутую
границу. Выберите существующий test target; новый regression нужен, когда
существующее покрытие не ловит конкретный дефект.

После правки:

1. Выполните проверки по матрице ниже.
2. Обновите ближайший справочник и примеры, если изменилось поведение.
3. Проверьте `git diff --check` и создайте отдельный commit.
4. В результате перечислите выполненные проверки и непройденные применимые
   проверки с причиной.

Не запускайте подряд focused, package и workspace suites, если последний
набор уже включает нужные сценарии. Успешную проверку повторяют после новых
затрагивающих её изменений или для разбора конкретной ошибки.
Если полный прогон завершился с отдельными failures и изменены только эти
tests/fixtures, повторите упавшие targets. Успешные targets сохраняют evidence;
повторный workspace нужен при новой общей production-правке.
CI отключён; проверки выполняются локально. Manual dogfood — добровольная
[диагностика](dogfood-gate.md), без обязательного места в последовательности.

## Evidence Matrix

Матрица выбирается по изменённому поведению. Каталог targets ниже помогает
найти проверку; он не является списком команд для каждого изменения.

| Изменение | Достаточная проверка |
|---|---|
| Документация, комментарии | Содержание, локальные ссылки, `git diff --check` |
| Скрипт portable-сборки | Реальный `python3 scripts/portable.py --debug в Proteus-desktop`; launcher и `install-shortcut` из собранной папки |
| Prompt/config без schema change | Загрузка профиля; при подключении готового модуля — один целевой invocation; init/install только при изменении упаковки |
| Чистка tests | Изменённые test targets и сохранённые behavior checks |
| Локальный helper | Затронутый test target |
| Общий DTO/contract | Producers/consumers и полный Rust gate; затронутые клиенты отдельно |
| Assembly/config wiring | Plan → registry/topology, atomic reload, `module_swap`, `doctor` |
| Process protocol/slot adapter | Protocol/conformance, реальный модуль, `module_swap`; failure/restart/cancel по изменению |
| Component sandbox/OS launch | Реальные Rust/Python workers: grants, protected fixture вне grants, network deny, teardown descendants и fail-closed; config → registry/tool в `component_sandbox` |
| Module implementation | Module target; реальный invocation при изменении process boundary или side effects |
| Tool/policy | Поведение tool и общий registry/policy/approval path по изменённой ветке |
| Workflow/runtime | Canonical journal и replay в поддержанной границе; cold history/terminal при изменении recovery |
| Agent control | Real process peers, адресная доставка/отмена и независимые child configs |
| HTTP/session | Затронутые handlers; reconnect/cold history/auth/SSE при изменении этих границ |
| MCP/ACP | Реальный stdio peer; prompt/tools/approval/cancel и cold history по изменению |
| Клиент | `python3 scripts/test.py` в Proteus-desktop; при UX change — ручной запуск окна |
| Расширения клиента | Tests `declarative`/`extensions`; пример `Proteus-desktop/examples/extensions` в живом окне при изменении протокола |
| Запуск приложения и portable | Запуск backend, завершение вместе с окном, portable build и запуск собранной папки |

Для небольшой правки профиля используйте уже собранный актуальный executable.
Подключение существующего модуля не является изменением runtime wiring:
Cargo tests, пересборка reference-модуля, полный gate и portable build для
такой правки не нужны. Расширяйте проверку только ради конкретной неразрешённой
проблемы; долгую сборку или расширение задачи сначала обсуждайте с владельцем.
Несвязанные ошибки сообщайте отдельно, не исследуя и не исправляя без запроса.

Полный Rust gate нужен для общих contracts, runtime wiring, зависимостей,
изменений взаимодействующих crates, интеграции и release:

```bash
cargo fmt --all --check
./scripts/test.py full
git diff --check
```

Для локальной правки используйте, например,
`./scripts/test.py -p proteus-core --lib <filter>` или
`./scripts/test.py -p proteus-core --test <target> <filter>`.
`cargo check` не заменяет поведенческие tests.

## Быстрая Локальная Проверка

`scripts/test.py` принимает обычные Cargo arguments с явным `-p`, а `full`
выбирает весь workspace. Для локального module/helper выбирайте package или
filter из матрицы; полный прогон не нужен после каждого изменения.

Runner готовит reference-модуль из отдельного Proteus-modules через
`scripts/build_module.py` и передаёт его фактический Cargo artifact path в
`PROTEUS_TEST_REFERENCE_MODULE`. Core fixtures не запускают Cargo и не
угадывают binary в target. Явный `PROTEUS_TEST_REFERENCE_MODULE` используется
и в полном gate; его свежесть обеспечивает вызывающий сценарий. Отсутствующий
файл — ошибка. При подготовке JS fixture используются `hook/dcp` отдельного
репозитория и переменная `PROTEUS_TEST_DCP_MODULE`.

`full` теперь выбирает только четыре crates harness. Реализации проверяются
своим `python3 scripts/test.py full` в Proteus-modules. В приложении используется
`python3 scripts/test.py` из Proteus-desktop: он подготавливает отдельный набор
расширений, затем проверяет GPUI workspace и client-common.

Defaults runner-а: два build jobs, четыре test threads, loopback в `NO_PROXY`
и отключённые Python bytecode caches. Явные environment settings сохраняются.
После изменения build settings первая сборка заполняет cache заново; скорость
тёплого прогона измеряется отдельно от этой разовой стоимости.

Dev/test сохраняют line tables для backtraces без тяжёлой variable/type debug
информации. Для отладки переменных можно задать `CARGO_PROFILE_DEV_DEBUG=2`
и `CARGO_PROFILE_TEST_DEBUG=2`; смена этих settings требует пересборки.
На `x86_64-unknown-linux-gnu` настроен установленный `lld` через `cc`;
для Linux development требуется executable `ld.lld`. Остальные targets,
включая WASM, используют свои linker settings.

## Каталог Проверок

Имена в таблицах — package, integration target или Rust module filter.
Точные cases и fixtures находятся в указанных исходниках; отдельный пересказ
каждого сценария в справочнике не поддерживается.

### Process И Modules

Context build проверяют `proteus-core --lib bound_context`,
`core::session_journal::context_tests` и `workflow_replay::tests::context_builds`:
общая запись прямого/host вызова, model-free success/error, full bundle и цепочка
ошибки, concurrent inputs, лишний/изменённый/пропущенный вызов даже при caught
error, pending и cold read после interruption. Process wire/authority не меняются;
подмена context implementation продолжает проверяться `module_swap` и profile swap.

| Граница | Target / исходники |
|---|---|
| Framing, child lifecycle, bounded transport | `proteus-process-host`, включая собственный runner `tests/session.rs` |
| Restricted Linux component launch | [`proteus-process-host --test sandbox`](../../crates/proteus-process-host/tests/sandbox.rs), [`proteus-core --test component_sandbox`](../../crates/proteus-core/tests/component_sandbox.rs); strict config — `proteus-core --lib process_adapters::sandbox` |
| RPC envelope, wire ID, handshake, export identity, callback authority, multiplexing | `proteus-module-protocol` (`v3::frame`, `v3::wire_id`), `tests/broker_v3.rs` |
| Подмена implementation, structural absence, shared component и failure/restart | [`proteus-core --test module_swap`](../../crates/proteus-core/tests/module_swap.rs) |
| Reference exports, descriptors, tools и callbacks | [`proteus-reference-module --test conformance`](https://github.com/Proteus-harness/Proteus-modules/blob/main/components/process-module/tests/conformance.rs) |
| Ordered hooks, actual effect и следующий живой turn после interruption, replay Success/Error | [`proteus-core --test hook_runtime`](../../crates/proteus-core/tests/hook_runtime.rs) |
| JS/TS hook SDK, перенос handlers, multiplexing и targeted cancel | `node --test examples/modules/hook-process/tests/*.test.mjs`; настоящие slot/journal/replay — `hook_runtime::js_ports` и `hook_runtime::review` |
| Model process, catalog/quota и cancellation | `proteus-core --test model_process`, `proteus-reference-module --test model_exports` |
| Skills/MCP/Agent Plugins management и автоподхват профиля | `proteus-core --lib app_server::http::tests::addons`, `app_server::profile_watch`, `core::agent_plugins`; загрузка и фильтрация — `skill-pack` |
| Execution attribution и операции без chat identity | `proteus-core --test execution_boundary`, `core::runtime::tests::execution`, `core::bound_model`, `core::bound_tools` |
| Один process с callback-связанными exports и journal/replay | `proteus-reference-module --test topology_journal` |
| Patch transaction через заменяемые process tools | `proteus-reference-module --test patch_transaction`; runtime workdir/freeform — `module_swap::patch` |

Для изменения restricted launcher Linux gate использует настоящий `/usr/bin/bwrap`;
недоступный backend или запрещённые user namespaces — failure, а не успешный skip.
При изменении `read_executable` дополнительно проверяются точный child `PATH`,
relative/absolute command и canonical `cwd`, отсутствие parent-directory grant,
отсутствие повторного PATH lookup, executable внутри read-only runtime и отказ
при пересечении с writable tree. Pin файла не доказывает неизменность его bytes
при внешней записи в тот же inode. `doctor` должен проверять команду с тем же
child environment.

Рабочий `restricted` проверяется как конкретная сборка: шесть single-export
components и их OS-grants, сохранённые tools/DCP/роли, assets установленного
CLI и перенесённого portable. Runtime smoke использует синтетическую модель и
временные каталоги личного состояния. CLI installer не поставляет DCP, поэтому
его smoke исключает DCP overlay; полный runtime test и portable проверяют оба
DCP exports вместе с restricted components. Успешный fake turn не является
проверкой живого provider или оценкой качества/стоимости. Default и `codex`
проверяются отдельно на сохранение прежней композиции.

Fixture содержит только синтетические данные. Targeted cooperative cancel
проверяется отдельно от teardown generation: он не обещает очистку OS descendants.
Команды для focused запуска:

```bash
./scripts/test.py -p proteus-process-host --test sandbox
./scripts/test.py -p proteus-process-host --lib
./scripts/test.py -p proteus-core --test config_profiles
./scripts/test.py -p proteus-core --test component_sandbox
./scripts/test.py -p proteus-core --lib process_adapters::sandbox
```

Новый callback проходит единую authority table и dispatcher всего slot.
Проверяются разрешённый вызов и отказ из другого slot; привилегия конкретного
reference `module_id` не является корректным evidence.
Protocol tests покрывают malformed/missing/unknown fields, неверные ids и
exports, запрещённые методы, oversized frames, child exit, module error,
timeout/cancel. Producer, consumer и fixtures меняются вместе; старый reader
ради compatibility не добавляется.

Conformance CLI без probe проверяет handshake, но не поведение slot.
Команды для внешних Python examples находятся рядом с ними:
[search](../../examples/modules/search-process/README.md),
[tgrep search](../../examples/modules/tgrep-search/README.md),
[preparation hook](../../examples/modules/context-preparation-process/README.md),
[workflow](../../examples/modules/agent-worker/README.md).

### Runtime, Journal И Replay

Для runtime-поведения источник истины — canonical journal:

- prompt replay сверяет post-shaping model request;
- workflow replay сверяет orchestration на записанных outcomes;
- cold `/history` подтверждает durable projection;
- `TurnSettled` фиксирует terminal state.

Workflow replay поддерживает root `Success`/`Error` в описанной
[границе](../architecture/canonical-turn-data.md#workflow-replay-v0).
Внешние `Canceled`/`Timeout` проверяются через settlement и cold history.
Replay должен обходиться без исходного model/tool effect и не менять source
journal. Для checkpoints проверяются history revision, точный
`execution_call`, выбранные results и их положение относительно model/tool
facts; совпавший final output не заменяет эти проверки.

| Граница | Target / filter |
|---|---|
| History, settlement, recovery, redaction | `proteus-core --lib core::session_journal`, `core::session_store`, `core::runtime` |
| Единственный OS writer | `proteus-core --test session_writer_lock` |
| Prompt/workflow replay | `proteus-core --lib core::prompt_replay`, `core::workflow_replay` |
| Codex stream/retry/tool progress/cold resume | `proteus-reference-module --test codex_model_resume` |
| Context preparation и совместимость стратегий | `codex-context-preparation`, `context-pack`, module targets `codex_compaction`, `context_preparation_interop` |
| Model-free controller | `coding-workflow project_check`, module target `project_check_workflow` |
| Frozen admission и run intent | `core::runtime::tests::snapshot_atomicity`, `coding-workflow intents` |
| Изображения: process workflow, cold resume, capability refusal, replay | `proteus-core --test execution_boundary images::`, provider encoding в `model-pack`, сохранение parts в `codex-context-preparation` |
| Process peers и их tool surfaces | Core targets `process_agent_control`, `process_agent_pool` |

Storage recovery проверяется отдельно от steady-state append: полный scan
выполняется при открытии writer, а не при каждом record. Redaction сохраняет
JSON schemas и удаляет credentials из metadata/arguments. Сокращение проверок
не является основанием убирать `sync_data`, checkpoint или policy contracts.

Pinned upstream revision, подтверждённые срезы и ограничения реконструкции
собраны в [codex-baseline.md](codex-baseline.md) и module `UPSTREAM.md`.
Новый upstream output сначала классифицируется как parity change,
unsupported capability или намеренный divergence. Snapshot не обновляется
вслепую. Replay проверяет эквивалентность; качество и расход на живых задачах
проверяются отдельным eval по [roadmap](../product/roadmap.md).

### Config, Tools И Transport

| Граница | Target / filter |
|---|---|
| TOML/JSON, includes, profiles | `proteus-core --test config_profiles`, `core::config`, `core::assembly` |
| Выбор context implementation | `proteus-reference-module --test context_profile_swap` |
| CLI dispatch до загрузки config | `proteus-core --test cli_dispatch` |
| MCP discovery/result mapping/cancel/restart | `proteus-core --test mcp_client` |
| ACP prompt/approval/cancel, model/mode selectors | `proteus-core --test acp_server`, `--lib app_server::acp` |
| HTTP адресация, очереди, SSE и cold reads | `proteus-core --lib app_server::http` |
| Live projections и client revisions | `app_server::events`, `app_server::turn_progress`, `proteus-client-common` |
| OAuth/Responses/Anthropic mapping и retries | `model-pack`; module targets `auth_commands`, `model_exports` |
| Terminal processes | `shell-tool`; модуль `codex_model_resume terminal::` |

Изменённые configs должны загружаться через текущий reader. При изменении
schema обновляются все tracked producers/consumers/examples. Для проверки
сборки без model request используйте `doctor`, `modules list` или `tools list`
на representative profile; reference-модуль должен быть собран и доступен
в `PATH`.

Tool проверяется по собственной semantics: input, safety, visibility,
allow/ask/deny, path, timeout/cancel или output limits — когда эта ветка
затронута. Process tool проходит `list`/`invoke`, а runtime-вызов — общий
`BoundTools -> ToolRegistry -> policy`. Workflow вызывает tools через host,
а не исполняет команды самостоятельно.

Fixtures используют локальные HTTP/stdio peers, без live credentials.
Runner явно собирает reference-модуль до Core tests; это не production dependency
Core на reference crates. Shell fixtures разбирают JSON-RPC id JSON-парсером,
не полагаясь на порядок ключей.

### Клиенты И Установка

Клиент и `Proteus-desktop/crates/client-common` исключены из root workspace; клиент собирается
своим toolchain из `Proteus-desktop/rust-toolchain.toml`:

```bash
cargo test --manifest-path Proteus-desktop/Cargo.toml
cargo test --manifest-path Proteus-desktop/crates/client-common/Cargo.toml
```

Второй target нужен только при правке общих правил. При изменении shared
client DTO в `proteus-contracts` клиент проверяется вместе с producers.

Новая переносимая сборка (`python3 Proteus-desktop/scripts/portable.py`) нужна
при проверке упаковки, запуска или по запросу владельца. Не запускайте её
после каждой правки окна. Общие Rust gates выбирайте по затронутой границе,
а не по факту запуска приложения.

- Изолированная проверка installer: `./scripts/install-smoke.sh`.
  Она сама собирает и устанавливает оба executable во временные каталоги,
  проверяет fake turn, внешний Python component и process peers. Отдельная
  предварительная установка для этого smoke не нужна.
  `sh scripts/install-smoke-env.sh` выполняет тот же сценарий с подставным
  внешним `PROTEUS_CONFIG_PATH` и проверяет, что его файл не изменился.
  Smoke снимает этот override и направляет `init` в явный временный config.
  Process fixtures привязаны к установленным executable из `current`:
  shell launchers требуют пользовательский env, который component fixture
  намеренно не наследует. Launchers проверяются отдельными CLI этапами smoke.

Для docs-only правки этих руководств достаточно проверки содержания и ссылок;
пересборка клиента или установка не требуется.

## Ordered Hook Evidence

Самостоятельный DCP Node component проверяется `npm test` в
`Proteus-modules/hook/dcp`. Runner перед `full`, общим core integration scope и
целевыми `hook_runtime`/`module_swap`/`context_profile_swap` выполняет locked
`npm ci --ignore-scripts` и build,
передаёт свежий путь через `PROTEUS_TEST_DCP_MODULE`; сборок внутри Rust tests нет.
`hook_runtime` связывает реальные hook/tool exports и scripted process model:
outgoing context отличается, cold history сохраняет оригиналы, replay `Success`
и `Error` не вызывает model/DCP и не меняет package-owned state. Live-model
dogfood/eval проверяется отдельно: scripted evidence не доказывает полезность
или качество summary на повседневных задачах.

`hook_runtime::dcp::composition` проверяет DCP вместе с реально сработавшими
Rust Codex и Python suffix preparation hooks. Сначала `compress` меняет только outgoing
view; затем preparation hook предлагает удалить prefix history, сохраняя текущий user id. Проверяются
checkpoint, отсутствие stale pruning оставленного user в следующем model request,
новый runtime/process с той же session и сохранённым DCP state, cold transcript
и workflow replay без повторного `compress`, model calls или записи state.
Node `composition.test.mjs` отдельно проверяет exact file DTO protection,
инвалидацию потерявших ссылки блоков и зависимых родителей, сохранение валидных
вложенных блоков и одинаковое согласование state в живом и новом component.

## Slash-команды

`proteus-core --lib app_server::http::tests::slash` проверяет общий каталог,
prompt expansion и настоящую process-команду: allow/deny approval, idle admission,
cancel и cold journal без Turn/model call. `hook_runtime` проверяет DCP management
и replay без повторного изменения package state; `npm test` — оригинальные
stats/context/decompress handlers и восстановление view после restart.
User-only tool не попадает в model catalog и отклоняется model-request validation.

## Typed Hook Boundary

Отбор tools через `BeforeToolSelection` проверяется существующим
`module_swap::tool_selection`: реальные Rust/Python hooks, ordered narrowing,
metadata, запрещённое восстановление ранее скрытого tool и неизменность specs.
Workflow replay сверяет полный selection input и outcome, включая failure
без следующего model request и schemas скрытых tools. Standalone selection
проверяет отсутствие модели/conversation и общий policy/cancellation path.

Для `hook/v6` проверяются strict DTO/response validation, одинаковая authority
с общими model/tool callbacks по actionable phase, config order и отсутствие duplicate/unknown selection.
Boundary chain покрывает A→B/B→A, pre-effect failure, actual tool outcome
при post-effect failure, targeted cancellation и component restart. Config
builder roundtrip сохраняет order при изменении других slots. Изменение model
request/tool result проверяется canonical journal и workflow replay в
поддержанной границе; записанные transformed outcomes не применяются повторно.
`context_preparation_interop` проверяет model-aware Codex и model-free Python с
nullable request и detached attribution. Failed/unchanged preparation также
сохраняет hook trace и owner/parent tree дочерних операций; replay потребляет
его ровно один раз, без требования changed checkpoint. Объём production gate
для новой общей границы остаётся full workspace плюс `module_swap`.

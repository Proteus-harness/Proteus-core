# Experimental Rust Module SDK

`proteus-module-sdk 0.1.0-alpha.2` — worker-side библиотека для самостоятельных
Rust executable, реализующих процессные слоты Proteus. Это SDK модулей Core,
а не SDK расширений Desktop. Готовый модуль работает без Python и исходников
reference-модуля.

## Публичная Граница

Этот alpha фиксирует wire v3 и typed adapter `tool/v6`. Версия SDK, версия
общего wire и версия slot-контракта — отдельные величины. Между alpha API
может меняться; закрепляйте точную ревизию и проверяйте conformance при обновлении.
Проект не обещает совместимость всех будущих SDK с любым старым Core.

Публичные точки входа:

- `run(factory)` обслуживает stdin/stdout; `serve(reader, writer, factory)`
  использует тот же runtime с явно переданными потоками.
- `Export` — один уже связанный export: manifest и invocation handler.
  Factory получает canonical `ProcessComponentExportInitialize`, проверяет
  module id и config, затем создаёт только запрошенную реализацию.
- `tool::ToolModule` определяет canonical tool definitions и typed invocation;
  `tool::ToolExport` адаптирует их к существующим `list` и `invoke`.
- `hosts` содержит invocation-scoped bridges существующих slot contracts.
  Tool handler получает `ToolModuleHost`, включая cooperative cancellation,
  чтение разговора и host-routed tool/model operations.
- `contracts` и `protocol` реэкспортируют канонические crates. Используйте их,
  чтобы не смешивать Rust DTO из разных Git/path источников одной библиотеки.

Собственная реализация не копирует JSON-RPC dispatch. SDK обслуживает strict
handshake, generation-scoped IDs, concurrent и nested invocations, маршрутизацию
ответов host callbacks и адресную отмену. Он извлечён из worker runtime
reference-модуля; reference использует этот же код, сохраняя свои adapters,
registry, конфигурацию и алгоритмы в Proteus-modules.

Другие slots можно обслуживать через `Export`, используя их существующие
canonical DTO и helpers. В этом alpha удобный typed adapter и самостоятельный
starter поставляются для tool. Новый registry, ABI или самостоятельные
language-specific контракты не вводятся.

## Начать Со Starter

Скопируйте [examples/modules/rust-tool](../../examples/modules/rust-tool/README.md)
в отдельный каталог. Его `Cargo.toml` имеет собственный workspace и точную
Git dependency SDK. `main.rs` показывает минимальный путь:

1. Factory отклоняет неизвестные `slot/module_id` и разбирает config.
2. `ToolModule::definitions` объявляет tool spec и его доступность.
3. `ToolModule::invoke` получает `ProcessToolInvokeInput` с `ToolCall`, cwd,
   обязательной execution attribution и settings навыков. Разговорная identity
   для detached операции не выдумывается.
4. `ToolExport` оборачивает ответы в canonical envelope, проверяет schema
   аргументов и соответствие `ToolResult.call_id` исходному вызову.
5. `run` обслуживает процесс; Core подключает executable явным
   `components.<id>.exports.tool.<module_id>` и `tools.enabled`.

Ни factory, ни SDK не выбирают default implementation и не скачивают пакеты.
Установка executable и запись профиля остаются отдельным шагом доставки.

## Ошибки, Отмена И Host Calls

Возвращайте обычную ошибку операции tool через `Ok(ToolResult::error(...))`:
это успешный RPC с `ok=false`. Ошибка выполнения самого module handler через
`Err` становится terminal JSON-RPC error; следующий invocation может
использовать тот же component. Каноническая `ModelFailure`, полученная через
host bridge, сохраняется в error data, как у reference worker.

Долгая работа должна периодически проверять `host.is_cancelled()`. Полученная
отмена относится к конкретному invocation; SDK возвращает код `-32800`,
остальные текущие вызовы продолжаются. При закрытии входного потока или protocol error SDK также отменяет active
invocations и пробуждает ожидающие host callbacks; новые callbacks отвергаются.
Непрерываемая работа и блокирующие OS
операции не становятся отменяемыми автоматически: timeout и принудительное
завершение generation принадлежат host. Один component делит process lifecycle
и failure domain; пределы worker — 32 active invocations и 256 pending callbacks.

Host calls идут только через текущую invocation identity. Вложенный tool/model
вызов маршрутизируется host, не напрямую к соседней реализации. Authority
вычисляется по активному slot и контексту вызова; объединения прав exports нет.
SDK не является sandbox: OS-права задаются launch policy. Для независимой
изоляции используйте отдельный component с соответствующими grants.

Stdout занят newline-delimited JSON-RPC; пользовательские логи направляйте в
stderr. Реализации экспортов вызываются concurrent и обязаны синхронизировать
собственное разделяемое состояние. Не помещайте конкретные tools или providers
в crate SDK.

## Проверка

```bash
cargo test --locked -p proteus-module-sdk
```

Tests проверяют настоящий двусторонний wire: handshake, list и detached
invocation, module/domain errors, host callback с правильной identity,
reentrant dispatch и адресную отмену одного concurrent вызова с продолжением
другого. Неизвестные версии, identities и config отвергаются.

Starter отдельно собирается как внешний Cargo workspace и проходит публичный
`proteus-component-conformance`: команды приведены в его README. Это проверка
реального executable без reference source. Общие runtime gates и module swap
по-прежнему обязательны при изменении контрактов или общего wiring.

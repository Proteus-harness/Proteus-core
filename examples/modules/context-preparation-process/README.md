# Process Context Preparation Hook

`prepare.py` — dependency-free пример внешнего preparation hook. Python не
является частью контракта: процесс можно написать на любом языке, который
читает и пишет newline-delimited JSON-RPC 2.0.

Из корня репозитория модуль подключается так:

```toml
[modules]
hooks = ["python_suffix"]

[components.python-compactor]
command = "python3"
args = ["examples/modules/context-preparation-process/prepare.py"]

[components.python-compactor.exports.hook.python_suffix]
timeout_ms = 30000

[module_config.hook.python_suffix]
trigger_messages = 12
retain_user_turns = 2
```

Процесс проверяет `module_config.hook.python_suffix` при initialize и
сохраняет неизменяемые настройки своего единственного export. Неизвестные
ключи, нулевые и отрицательные значения завершают handshake ошибкой.
Алгоритм получает настройки отдельно от `ContextPreparationInput`; invoke
не может переопределить настройки запуска.

Сам алгоритм `compact(input, strategy)` реализован как pure transform;
его можно вызвать отдельно с явными настройками. Он не использует разрешённый contract-ом
`host.model.complete`. Strategy сохраняет canonical context и suffix от одного
из последних user turns. Это проверяемый пример протокола, а не качественная
замена model-aware `modules.hooks = ["codex_compact"]`.

Контекст определяется по canonical parts: непустое сообщение, у которого
все parts имеют `scope = "request"`. Поле `name` не участвует: workflow может
выбрать любое имя или не задавать его; user message с именем `context` остаётся
обычным user turn. Отсутствующий или неизвестный scope — ошибка входа.

`summary_source` и `skipped_reason` возвращаются явными полями
`ContextPreparationOutput`, а не ключами `metadata`. Strategy считает сообщения, не
токены, поэтому оценки и `trigger_tokens` остаются `null`. Общий отчёт берёт
числа сообщений из input/output, а исходную оценку токенов — из input, если
она была передана; module-specific `metadata` не переопределяет эти значения.

Проверка общей семантики с Rust compactor через реальные worker processes:

```bash
python3 scripts/test.py -p proteus-reference-module --test context_preparation_interop
```

Процесс получает очищенное окружение с `PATH`; дополнительные имена
родительских переменных перечисляются в `env_allowlist`, literal значения — в
`env`.

Worker использует общий component protocol v3 и hook contract v6.
На `BeforeContext` возвращает typed `context_preparation` candidate; workflow
принимает его через общий checkpoint. Остальные фазы возвращают `Continue`.
Model request nullable: алгоритм работает и без модели. Handshake
можно проверить отдельно от core:

```bash
cargo run -p proteus-module-protocol --features conformance-cli --bin proteus-component-conformance -- \
  --component-id python-compactor \
  --export '{"slot":"hook","module_id":"python_suffix","contract_version":"v6","module_config":{"trigger_messages":12,"retain_user_turns":2}}' \
  -- python3 examples/modules/context-preparation-process/prepare.py
```

Это только protocol handshake. Slot-level probe и runtime swap проверяются
component conformance и `module_swap`, потому что корректный `ContextPreparationInput`
содержит canonical model/message DTO.

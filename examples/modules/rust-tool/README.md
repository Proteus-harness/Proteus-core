# Самостоятельный Rust Tool

Это starter внешнего процессного модуля на experimental
`proteus-module-sdk 0.1.0-alpha.2`: wire v3 и `tool/v6`. Он не зависит от
reference executable, исходников Proteus-modules, Desktop или Python SDK.
Python используется только в необязательной команде проверки `smoke.py`.

Каталог имеет собственный `[workspace]`: его можно скопировать из Core в
отдельный репозиторий. SDK закреплён на точный Git commit; канонические DTO
импортируются через `proteus_module_sdk::contracts`, поэтому для них не нужна
отдельная dependency с потенциально другой Rust source identity.

## Сборка И Подключение

Из этого каталога:

```bash
cargo build --locked
```

Starter закрепляет Rust/Cargo 1.95.0; нужен Git-доступ к ревизии SDK.
Готовый executable не требует Rust, Python или соседних репозиториев.
Укажите абсолютный путь к нему в профиле Core:

```toml
[tools]
enabled = ["echo"]

[components.echo]
command = "/absolute/path/to/rust-tool/target/debug/proteus-rust-tool-example"

[components.echo.exports.tool."example.echo"]
timeout_ms = 30000

[module_config.tool."example.echo"]
prefix = "hello: "
```

Это фрагмент профиля: добавьте его к своей существующей композиции с model,
workflow и context. Если `[tools]` уже существует, добавьте `echo` к её
`enabled`, не создавая повторную таблицу. Весь TOML-профиль для работы агента
здесь не подменяется. Handshake запрашивает именно `tool/example.echo`;
неизвестная identity, версия контракта или поле конфигурации завершают запуск
ошибкой. Аргумент вызова — `{"text":"world"}`, результат — `hello: world`.

## Проверка Готового Executable

Из корня Core соберите публичный conformance CLI один раз:

```bash
cargo build --locked -p proteus-module-protocol --features conformance-cli --bin proteus-component-conformance
python3 examples/modules/rust-tool/smoke.py \
  --conformance target/debug/proteus-component-conformance \
  --module examples/modules/rust-tool/target/debug/proteus-rust-tool-example
```

Команда запускает настоящий внешний executable и проверяет handshake,
`list`, detached `invoke`, настройку prefix и отклонение неверных аргументов.
Она ничего не собирает и не устанавливает. CLI можно передать по любому
пути; соседний checkout Modules не нужен. Отмена, callback routing и
reentrancy SDK проверяются `cargo test --locked -p proteus-module-sdk`.

Как написать свою реализацию и где проходит публичная граница:
[руководство SDK](https://github.com/Proteus-harness/Proteus-core/blob/main/docs/guides/rust-module-sdk.md).

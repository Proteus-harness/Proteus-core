# DCP — Отложенный Эксперимент

Здесь сохранены прежний профиль, overlay и снимок интеграционных fixtures DCP.
Они исключены из штатных профилей, installer и Cargo test targets. Rust fixtures
сохраняют контекст прежнего `hook_runtime` и требуют отдельного подключения
при возобновлении эксперимента; текущие runners их не готовят и не запускают.

Сам плагин, его Node tests, lockfile и лицензии находятся в
[Proteus-modules/research/dcp](https://github.com/Proteus-harness/Proteus-modules/tree/main/research/dcp).
Упаковка прежнего portable сохранена отдельно в
[Proteus-desktop/examples/research/dcp](https://github.com/Proteus-harness/Proteus-desktop/tree/main/examples/research/dcp).

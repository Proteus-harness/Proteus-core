# Политика Безопасности

## Поддерживаемая Линия

Proteus развивается из текущей ветки `main`; отдельная поддерживаемая release
line пока не объявлена. Backport, срок реакции и совместимость старых
wire/config/storage форматов не гарантируются.

## Как Сообщить О Проблеме

Не публикуйте рабочий exploit, token, API key, содержимое приватного
репозитория или другой секрет в открытом issue. Используйте приватный канал
владельца репозитория или GitHub private vulnerability report, если он доступен.
В первом сообщении достаточно указать:

- затронутую версию или commit;
- минимальные шаги воспроизведения без секретов;
- ожидаемое и фактическое поведение;
- возможное влияние;
- известный workaround.

Обычные ошибки без чувствительных деталей можно оформлять публичным issue.

## Текущая Граница Доверия

Configured process component по умолчанию — доверенный локальный executable.
Он работает с OS-правами пользователя Proteus. Очищенный environment, strict
handshake и callback authority ограничивают protocol surface, но не прямые
OS-действия worker. Не подключайте недоверенную команду в trusted режиме.

Opt-in `components.<id>.sandbox.mode = "restricted"` ограничивает один configured
export под Linux через `/usr/bin/bwrap`: явные filesystem grants, сеть закрыта
по умолчанию, без unsandboxed fallback. Режим действует на persistent process
generation; cooperative invocation cancel не доказывает завершение её OS
descendants. MCP, LSP, Proteus peers и расширения Desktop этот режим не
наследуют. Подробные условия и пример — в
[security-and-policy.md](docs/guides/security-and-policy.md#ограниченный-запуск-components-под-linux).

Вызовы model-visible tools из process workflow возвращаются в core и проходят
общий путь `ToolRegistry -> ApprovalPolicy -> ToolSafety -> Tool`. Это защищает
tool execution path, но не перехватывает произвольные OS-действия самого
worker-а.

HTTP app-server предназначен для loopback. Установленный wrapper включает
ephemeral session token по умолчанию; не публикуйте его порт в сеть и не
сохраняйте token в логах или `localStorage`.

Полная текущая модель, exec sandbox и известные ограничения описаны в
[docs/guides/security-and-policy.md](docs/guides/security-and-policy.md).

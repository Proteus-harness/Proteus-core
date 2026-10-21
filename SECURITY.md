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
descendants. Для stdio MCP server та же launch policy включается отдельно через
`tools.mcp_servers[].sandbox`; она ограничивает весь процесс сервера и все его
remote tools. При отмене или timeout MCP-вызова завершается вся generation
сервера. LSP, Proteus peers и расширения Desktop сохраняют отдельные launch paths.
Подробные условия и пример — в
[security-and-policy.md](docs/guides/security-and-policy.md#ограниченный-запуск-components-под-linux).

Вызовы model-visible tools из process workflow возвращаются в core и проходят
общий путь `ToolRegistry -> ApprovalPolicy -> ToolSafety -> Tool`. Это защищает
tool execution path, но не перехватывает произвольные OS-действия самого
worker-а.

HTTP app-server предназначен для loopback. Прямой `proteus server http` и
установленный CLI wrapper не включают token auth для loopback по умолчанию;
используйте `--token`, если он нужен. Non-loopback bind требует непустой token.
Desktop обычно запускает сервер со случайным session token. Не сохраняйте token
в логах или `localStorage`.

Полная текущая модель, exec sandbox и известные ограничения описаны в
[docs/guides/security-and-policy.md](docs/guides/security-and-policy.md).

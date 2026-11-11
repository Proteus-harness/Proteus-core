# Проверка восстановления после обрыва модели

`recovery_probe.py` запускает установленный Core и настоящие выбранные workflow
и shell tool handlers. В первой модели инструмент увеличивает файл `counter.txt`
с `0` до `1`. Следующий ответ содержит незавершённые текст и аргументы инструмента,
затем обрывается. Повторный запрос должен содержать сохранённый результат первого
инструмента. Последний ответ завершает ту же сессию и тот же turn.

Режим `--native-provider` использует настоящий Pi ModelRuntime и адаптер модели
профиля. Node preload возвращает локальный OpenAI SSE и обрывает body с
`UND_ERR_SOCKET`. Внешние обращения отклоняются самим preload и сетевой песочницей
`bwrap --unshare-net`. Для ModelRuntime создаётся синтетическая авторизация,
которую runner удаляет после выполнения. Этот режим проверяет классификацию
ошибки адаптером и восстановление в workflow одной цепочкой.

Команды из Proteus-core используют новый каталог результата.

```sh
python3 -B examples/performance/coding-harness/recovery_probe.py \
  --native-provider \
  --output target/performance/coding-harness/recovery-native
python3 -B examples/performance/coding-harness/recovery_probe.py \
  --mode terminal \
  --output target/performance/coding-harness/recovery-terminal
python3 -B examples/performance/coding-harness/recovery_probe.py \
  --mode exhausted \
  --output target/performance/coding-harness/recovery-exhausted
```

Режимы `terminal` и `exhausted` используют синтетический model/v13. Первый
возвращает терминальную ошибку, второй проверяет конечность бюджета повторов.
Оба сохраняют ранее выполненный эффект, а финальный статус задачи остаётся
ошибкой. Путь к другой готовой поставке задаётся `--portable`.

Для каждого профиля `proof.json` проверяет значение счётчика, единственные
`call_id` и result, отсутствие эффекта незавершённого вызова, число модельных
попыток и принятую историю. Затем выполняется offline replay с read-only
workspace, конфигурацией и исходным журналом. Повторное исполнение сравнивается
с canonical trace, SHA256 журнала и все файлы сессии должны сохраниться.

Отдельный `recovery_live_probe.py` требует явного `--live` и расходует настоящую
подписку Go, DeepSeek Flash/high. Он запускает один Codex-профиль с лимитом 230с.
Модель сама выбирает и выполняет единственный append; после принятого ToolResult
preload проверяет файл на диске, передаёт первые 8 байт настоящего HTTP/SSE ответа
и один раз обрывает чтение. Следующий ответ тоже приходит из настоящей модели.
В `proof.json` и `runtime/live-transport.jsonl` сохраняются хеши messages+tools,
идентификаторы результата и эффекта, число строк и финальный ответ, без ключа.
Это **инъекция транспортного сбоя на настоящем провайдере**, не наблюдение
самопроизвольного сетевого сбоя. Автоматического перезапуска всего опыта нет.

```sh
python3 -B examples/performance/coding-harness/recovery_live_probe.py --live \
  --output target/performance/coding-harness/recovery-live
```

Контрольный живой прогон `target/performance/coding-harness/recovery-live-controlled-01`
прошёл за **6,78с**: 3 настоящих HTTP200/SSE запроса Flash/high, одна сессия и один
turn, одна строка append, единственные call/effect/result с одним ID. Между
оборванным запросом и повтором совпал SHA256 messages+tools
`f29837f91c29d0c9bbb8924c705a379bd2ab74d4b1d8960de306dad4b3b3ad46`.
Реальная модель завершила ответом `RECOVERY_LIVE_OK`; `proof.json` содержит
`proof_ok=true`, `credential_leak_detected=false`, `auth_removed=true`.
Перед обрывом передано 8 реальных байт; содержимое этих байт отдельно не сохранено.

Запущенный preload сохранён как `runtime/executed-recovery_live_preload.mjs`,
SHA256 `347fc59ba5c77c9866ffbccd16ce95652afa14a9bb5235af7be9af5c42e861ec`.
В первом прогоне этот снимок восстановлен сразу после выполнения отменой
единственной последующей косметической правки; это явно отмечено в
`preload-provenance.json`. Сейчас runner копирует исходник до запуска.
Исходный лог первого прогона сохранён без правок: две записи `ERR_INVALID_STATE`
после `[DONE]` относятся к гонке `controller.close()` с отменой чтения потребителем,
что подтверждается предшествующими `real_sse_done`/`real_stream_completed`.
В журнале модели был ровно один транспортный сбой, внедрённый на запросе 2.
Правка устраняет ошибочную маркировку этой гонки, повторного живого прогона не было.

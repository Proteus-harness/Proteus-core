# Операции Hooks И Tools

Статус: **предложение архитектурной границы, не реализовано**. Подготовлено
по запросу владельца после сравнения Pi и OpenCode. Действующие версии,
authority и compactor slot остаются описаны в
[process-module-architecture.md](process-module-architecture.md).

Цель — дать процессным hooks и tools общие операции модели и инструментов,
а изменение рабочей истории выразить через предложение и принятие результата.
Это позволяет проверить удаление отдельного algorithm slot `compactor`,
сохранив независимо выбираемую стратегию через hook export. Нового слота,
реестра сервисов или native extension path предложение не вводит.

## Действующая И Предлагаемая Граница

| Ответственность | Сейчас | Предложение |
|---|---|---|
| Вызов модели из hook/tool | Host callback отсутствует | Общий execution-bound `host.model.complete` |
| Вызов другого tool | У hook/tool нет общего callback | `host.tools.execute` через существующий `BoundTools` |
| Изменение следующего model request | `BeforeModel → ModelContext` | Сохраняется как request-only преобразование |
| Алгоритм сжатия | Выбранный `compactor/v11` | Выбранный hook общей подготовки контекста, с model callback при необходимости |
| Принятие рабочей истории | Workflow history update/checkpoint | Та же обязанность workflow и host, с явным ответом о принятой истории |
| Восстановление исходного разговора | Canonical journal | Сохраняется; преобразование не удаляет исходные records |

`BeforeModel` сам по себе не становится правом записи истории. Workflow
держит собственные `persistent_messages`, model view и текущую пользовательскую
identity. Если изменить store за его спиной, следующий checkpoint может
восстановить старую копию. Принятый результат должен вернуться владельцу
этого состояния.

## Общие Обратные Вызовы

Каждый вызов использует captured assembly, execution scope, cancellation и
остаток срока родителя. Родительская связь назначается host, а не берётся из
opaque metadata. Возможности одинаковы для всех implementations одного slot
в одинаковом invocation context. Совмещение exports в одном component не
объединяет их права.

### Модель

`host.model.complete` принимает canonical request. Для hooks/tools он выполняет
вспомогательный вызов; workflow явно различает основной и вспомогательный
режим. Режим — typed семантика операции, а не проверка имени implementation.

В обоих режимах сохраняются canonical shaping, model/tool validation,
execution binding, journal, usage и отмена. Вспомогательный вызов:

- не потребляет root steering и не выдаёт свой ответ за основной ответ агента;
- возвращает ответ вызывающему модулю без автоматического append в conversation;
- не запускает повторно `BeforeModel` transformation chain;
- записывается как дочерняя операция, включая ошибку или отмену, даже если
  после него не было изменения истории.

Отказ от повторного запуска chain предотвращает
`BeforeModel → model → BeforeModel` recursion.
Это правило всех вспомогательных вызовов, а не исключение для summarizer.
Проверка model-visible tools при этом остаётся обязательной. Неподдержанный
provider request не исправляется специальным fallback.

Наличие callback не делает модель обязательной. Без выбранного model service
вызов завершается явной ошибкой; hook, который только обрезает хвост истории,
продолжает работать без модели. Новых streaming cursors предложение не добавляет.

### Инструменты

`host.tools.execute` вызывает выбранный tool через captured `ToolRegistry`,
policy, approval, safety и обычные before/after-tool hooks. Прямого вызова
implementation другого export нет. Вложенные результаты возвращаются родителю;
в основную историю они входят только через явное принятие workflow.

Для первой границы callback доступен tool invocation и actionable hook phases
подготовки контекста, `BeforeModel` и `BeforeStop`. В `BeforeTool`/`AfterTool`
он недоступен: повторный вход в ту же tool-hook chain не обходится пропуском
обработчиков. Lifecycle notifications не запускают новую дочернюю работу.
Model callback допустим в actionable phases, включая tool hooks, и использует
описанный выше вспомогательный режим.

Host отклоняет повтор той же registry tool identity в активной parent lineage,
даже с новым call ID. Обычный вложенный вызов другого tool проходит полный путь.
Ошибка не превращается в прямое исполнение без hooks или policy. Отдельная более
широкая reentrancy семантика потребовала бы своего доказательства.

Дочерняя операция не получает фиктивный canonical assistant tool call.
Conversation read, требующий такого owner, доступен только при реальном host
binding; без него возвращается явная ошибка. Read-only snapshot разговора и
чтение источника конкретного call — разные операции. Обычный tool без
разговорного контекста продолжает работать без такого чтения.

## Подготовка Контекста И Предложение Истории

Предлагается общая workflow operation `host.context.prepare` и typed hook phase
`BeforeContext`. Они обслуживают подготовку рабочего материала перед следующим
model request. Операция не выбирает алгоритм по `module_id`.

Семантический вход содержит:

- рабочие canonical messages и параметры подготовки, включая доступные оценки
  размера и лимиты выбранной модели;
- nullable canonical request template с instructions, model controls и cache
  settings, если алгоритм должен сохранить параметры предстоящего запроса;
- nullable conversation target, выданный host, с revision основания;
- явную область преобразуемых messages и защищённый suffix, когда алгоритм
  обрабатывает только часть working history;
- текущую пользовательскую identity и подтверждённые tool-result bindings,
  когда подготовка относится к разговору.

Результат обработчика — отсутствие изменения либо typed предложение:

```text
HistoryEditProposal
  bound target + base revision
  replacement messages
  explicit source -> replacement derivations
  typed preparation report
```

Имя DTO обозначает проектируемую семантику, не существующий wire API.
Request-only преобразование возвращает view без durable proposal. Частная
история работника также может преобразовываться, но не получает root target
по совпадению message IDs. Standalone operation не требует conversation или
model binding, если её алгоритм их не использует.

Tools могут запросить подготовку и сохранять данные для выбранного hook, как
делает DCP. Сам `host.context.prepare` вызывает workflow после завершения batch;
tool не получает права запустить его за спиной владельца истории. Если tool
возвращает предложение durable edit, оно должно быть отдельным typed полем
результата, не текстовым marker в `ToolResult.output`. Возвращённое предложение
ещё не является принятым изменением истории.

### Точка Принятия

Workflow вызывает preparation на безопасной границе: предыдущий model stream
закрыт, tool batch завершён, фактические результаты записаны. Это может быть
pre-turn подготовка до построения свежего context либо подготовка между
model rounds. Уведомление от tool не заставляет host выполнять её посреди batch.

Порядок:

1. Host привязывает target/revision и запускает ordered `BeforeContext` chain.
2. Каждый следующий hook видит проверенный candidate предыдущего. Identity
   validation учитывает исходное основание и все шаги: удаление message на
   одном шаге не разрешает переписать его старый ID на следующем.
3. Workflow получает итоговое предложение и формирует staged candidate отдельно
   от последней принятой working history, model view и user identity.
4. Общий checkpoint принимает результат. Acknowledgement возвращает фактически
   принятую canonical history и revision, включая host-added tool results.
5. Workflow заменяет принятое локальное состояние этим ответом и строит
   следующий request.

Для частной подготовки без conversation target возвращается проверенная local
projection. Root checkpoint не создаётся; сохранение частного состояния остаётся
обязанностью его владельца.

Изменения ordered chain принимаются одним checkpoint; промежуточные candidates
не публикуются как рабочая история. При ошибке workflow не продолжает sampling
из неподтверждённого candidate. Rejected candidate не переносится в terminal
`history_replacement`/`new_messages` или `WorkflowFailure.history`: settlement
не должен обходить отказ checkpoint. Уже выполненные вложенные model/tool
операции остаются фактами журнала и не откатываются вместе с предложением.

Если commit состоялся, но acknowledgement потерян, принятая история остаётся
в журнале. Host не заменяет её stale состоянием из failure/terminal output
workflow; cold read возвращает фактический accepted checkpoint.

Revision относится к принятой working history. Запись вложенного tool result
без root history binding сама по себе её не меняет. Изменение revision, включая
принятый steering или включённый в history tool result, отклоняет stale proposal
явно. Tool proposal из parallel batch может устареть до точки принятия;
для автоматического сжатия свежий proposal строит `BeforeContext` после batch.
Молчаливого rebase, повторного исполнения effects или last-writer-wins нет.
После commit отмена следующего sampling не отменяет принятую историю.

В pre-turn Codex summary input содержит старую историю без уже admitted
текущего пользователя. Workflow задаёт эту область явно; current user остаётся
защищённым suffix и возвращается неизменным в полный replacement перед commit.
Root target не разрешает hook преобразовать весь snapshot за пределами заданной
области. Свежий request context строится после принятия результата.

### Инварианты Истории

- Старые message/part IDs сохраняют своё содержимое и identity.
- Производное представление получает свежие IDs и явную связь с источником.
- Текущий admitted user сохраняется либо имеет явно допустимое производное
  представление; workflow получает актуальную identity.
- Request-scoped context не становится conversation history по имени сообщения.
- Tool results относятся только к реально записанным outcomes и точным bindings;
  replacement не придумывает результаты и не теряет незавершённые calls.
- Исходные journal records остаются доступны transcript, replay и диагностике.

Эти правила обобщают существующую validation рабочей истории. Внедрение должно
убрать привязку derivation validation к process slot `compactor`, а не отменить
саму проверку происхождения.

## Journal И Replay

Нынешний replay отделяет `Direct` от специальных `Compactor` exchanges и
подставляет записанный HookTrace без исполнения hook. Для общих callbacks этого
недостаточно: одного признака «вспомогательный запрос» также недостаточно.

Journal должен сохранять typed owner/parent связи операций:

```text
workflow
  -> hook/tool invocation
       -> model/tool invocation
       -> proposal
  -> accepted history checkpoint
```

Purpose определяет режим модельного вызова; owner определяет, кому принадлежит
результат при replay. Workflow-owned exchanges остаются в его последовательности.
При подстановке записанного hook/tool outcome replay проверяет и потребляет его
дочерние records ровно один раз, включая failed/unchanged preparation. Вложенные
model calls не занимают место следующего workflow request, а tools не исполняются
снова. Простое отбрасывание auxiliary records не является такой проверкой.

Cold resume читает принятую working history из journal. Request-only DCP view
по-прежнему восстанавливает выбранная implementation из своего состояния;
её состояние не объявляется частью общей history mutation автоматически.

## Две Формы Подключения

**DCP:** tool сохраняет summary/prune state, `BeforeModel` строит view. Durable
root history не меняется. Общие callbacks не требуют переносить его собственную
стратегию в host.

**Автоматическое сжатие Codex:** `BeforeContext` hook проверяет выбранный порог,
при необходимости делает auxiliary summary call и предлагает replacement.
Workflow сохраняет точный порядок pre-turn summary, свежего context и incoming
user; Core не распознаёт Codex и не выбирает его fallback. Детерминированный
Python suffix выполняет ту же preparation без model callback.

Выбор стратегии остаётся отдельным от workflow: профиль выбирает hook export.
После реализации и проверки этого пути `modules.compactor`, compactor export,
adapter и специальный callback dispatch удаляются вместе. Старые config/wire/
storage формы не сохраняются aliases или dual-read. Сейчас они не удалены.

## Что Подтверждает Границу

Реализация должна подтвердить DCP, автоматический Codex summary и model-free
suffix через общие операции, с отменой, ошибкой, cold history и replay. Отдельные
источники дефекта — цепочки `A → B`/`B → A`, stale revision, сохранение user/part
identity, helper recursion, вложенный tool approval и replay parent ownership.
Актуальность `module_swap` остаётся обязательной при изменении runtime границы.

При внедрении одновременно меняются contracts, authority, adapters, producers,
consumers, profiles и документы. DTO/wire versions назначаются этому изменению;
этот документ не объявляет новые версии реализованными. Общие gates выбираются
по [testing.md](../development/testing.md), а не заменяются чтением чужого кода.

## Проверенные Образцы

- Pi: [обычная custom compaction](https://github.com/earendil-works/pi/blob/96abe9a5ad45005cae34ed1c52d42f71fc88bedc/packages/coding-agent/examples/extensions/custom-compaction.ts),
  [typed boundary entries](https://github.com/earendil-works/pi/blob/96abe9a5ad45005cae34ed1c52d42f71fc88bedc/packages/coding-agent/src/core/extensions/types.ts#L958).
- OpenCode V2 dev: [plugin operations](https://github.com/anomalyco/opencode/blob/d21b0790a6c146815f28fea604e41e922f009590/packages/plugin/src/promise/plugin.ts),
  [prepared compaction result и publication](https://github.com/anomalyco/opencode/blob/d21b0790a6c146815f28fea604e41e922f009590/packages/core/src/session/compaction.ts#L392).

Это источники конкретных механизмов, не объявление совместимости Proteus с их
полным API. Предлагаемая process boundary, nested tool path и checkpoint rules
принадлежат Proteus.

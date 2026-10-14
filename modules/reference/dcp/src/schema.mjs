// Form description of the DCP settings for the host's config editor.
// Defaults mirror upstream `defaultConfig`; validation stays in config.mjs.

const field = (key, title, description, value, extra = {}) => ({
  key, title, description, value, default: null, required: false, advanced: false, unit: null, ...extra,
});
const bool = { type: "boolean" };
const count = { type: "integer", minimum: 1, maximum: null };
const text = { type: "string", multiline: false, secret: false };
const list = { type: "array", items: text };
const json = { type: "json" };
const object = (fields) => ({ type: "object", fields });
const choice = (options) => ({ type: "enum", options: options.map(([value, title]) => ({ value, title })) });
const limit = 'Число токенов или доля окна модели строкой вида "80%".';
const extraTools = "Добавляются к защищённым по умолчанию.";

const compress = object([
  field("mode", "Режим", "range сжимает участки переписки в сводки блоков; message (экспериментальный) — отдельные сообщения.",
    choice([["range", "Участки (range)"], ["message", "Сообщения (message)"]]), { default: "range" }),
  field("maxContextLimit", "Верхний порог", `Выше него DCP настойчиво предлагает сжатие. ${limit}`, json, { default: 100000 }),
  field("minContextLimit", "Нижний порог", `Ниже него напоминания о сжатии выключены. ${limit}`, json, { default: 50000 }),
  field("summaryBuffer", "Учитывать сводки", "Токены активных сводок расширяют верхний порог.", bool, { default: true }),
  field("nudgeFrequency", "Частота напоминаний", "Как часто срабатывает напоминание у порога: 1 — каждый запрос, 5 — каждый пятый.", count, { default: 5 }),
  field("iterationNudgeThreshold", "Напоминать после", "Сколько сообщений после последнего сообщения пользователя пройдёт до напоминаний.", count, { default: 15, unit: "сообщ." }),
  field("nudgeForce", "Сила после ответа пользователя", "Насколько вероятно сжатие после сообщения пользователя.",
    choice([["soft", "Мягко"], ["strong", "Сильно"]]), { default: "soft" }),
  field("protectedTools", "Защищённые инструменты", `Их результаты дописываются в сводку. ${extraTools}`, list),
  field("protectTags", "Тег <protect>", "Сохранять текст внутри <protect>…</protect> при сжатии.", bool, { default: false }),
  field("protectUserMessages", "Сообщения пользователя", "Сохранять сообщения пользователя дословно; большие вставки тогда не сжимаются.", bool, { default: false }),
  field("modelMaxLimits", "Верхний порог по моделям", `Объект "provider/model" → порог; заменяет общий. ${limit}`, json, { advanced: true }),
  field("modelMinLimits", "Нижний порог по моделям", `Объект "provider/model" → порог; заменяет общий. ${limit}`, json, { advanced: true }),
]);

const strategies = object([
  field("deduplication", "Повторы", "Убирать повторные вызовы инструмента с теми же аргументами.", object([
    field("enabled", "Включено", "", bool, { default: true }),
    field("protectedTools", "Защищённые инструменты", extraTools, list),
  ])),
  field("purgeErrors", "Ошибки", "Убирать входные данные упавших вызовов через несколько ходов.", object([
    field("enabled", "Включено", "", bool, { default: true }),
    field("turns", "Через", "", count, { default: 4, unit: "ходов" }),
    field("protectedTools", "Защищённые инструменты", extraTools, list),
  ])),
]);

const turnProtection = object([
  field("enabled", "Включено", "", bool, { default: false }),
  field("turns", "Ходов", "", count, { default: 4 }),
]);

export const configSchema = { fields: [
  field("compress", "Сжатие", "Инструмент сжатия контекста и напоминания модели.", compress),
  field("strategies", "Автоочистка", "Стратегии, которые убирают лишнее без участия модели.", strategies),
  field("turnProtection", "Защита свежих вызовов", "Не трогать вызовы инструментов заданное число ходов.", turnProtection),
  field("protectedFilePatterns", "Защищённые файлы", "Glob-шаблоны путей: операции с этими файлами не очищаются.", list),
  field("state_dir", "Каталог состояния", "Абсолютный путь, где DCP хранит состояние сессий. По умолчанию $XDG_DATA_HOME/Proteus-agent/modules/dcp.", text, { advanced: true }),
  field("debug", "Отладка", "Подробный журнал DCP.", bool, { default: false, advanced: true }),
] };

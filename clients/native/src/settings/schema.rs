use proteus_contracts::domain::{ConfigChoice, ConfigField, ConfigValueSchema};
use serde_json::Value;

/// How a text control reads and writes one module config value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextFormat {
    Integer {
        minimum: Option<i64>,
        maximum: Option<i64>,
    },
    Number,
    Line,
    Secret,
    Multiline,
    /// Array of single-line strings, one per line.
    Lines,
    Json,
}

impl TextFormat {
    pub fn is_multiline(self) -> bool {
        matches!(self, Self::Multiline | Self::Lines | Self::Json)
    }
}

#[derive(Clone, Copy, Debug)]
pub enum FieldKind<'a> {
    Switch,
    Choice(&'a [ConfigChoice]),
    MultiChoice(&'a [ConfigChoice]),
    Text(TextFormat),
}

pub fn field_kind(schema: &ConfigValueSchema) -> FieldKind<'_> {
    match schema {
        ConfigValueSchema::Boolean {} => FieldKind::Switch,
        ConfigValueSchema::Enum { options } => FieldKind::Choice(options),
        ConfigValueSchema::Integer { minimum, maximum } => FieldKind::Text(TextFormat::Integer {
            minimum: *minimum,
            maximum: *maximum,
        }),
        ConfigValueSchema::Number {} => FieldKind::Text(TextFormat::Number),
        ConfigValueSchema::String { secret: true, .. } => FieldKind::Text(TextFormat::Secret),
        ConfigValueSchema::String {
            multiline: true, ..
        } => FieldKind::Text(TextFormat::Multiline),
        ConfigValueSchema::String { .. } => FieldKind::Text(TextFormat::Line),
        ConfigValueSchema::Array { items } => match items.as_ref() {
            ConfigValueSchema::Enum { options } => FieldKind::MultiChoice(options),
            ConfigValueSchema::String {
                multiline: false,
                secret: false,
            } => FieldKind::Text(TextFormat::Lines),
            _ => FieldKind::Text(TextFormat::Json),
        },
        ConfigValueSchema::Object { .. } | ConfigValueSchema::Json {} => {
            FieldKind::Text(TextFormat::Json)
        }
    }
}

/// Stable identity of a choice value for comparisons and menu keys.
fn choice_key(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_default()
}

pub fn choice_title<'a>(options: &'a [ConfigChoice], value: &Value) -> Option<&'a str> {
    options
        .iter()
        .find(|choice| &choice.value == value)
        .map(|choice| choice.title.as_str())
}

pub fn format_value(format: TextFormat, value: &Value) -> String {
    match (format, value) {
        (_, Value::String(text)) if format != TextFormat::Json => text.clone(),
        (TextFormat::Lines, Value::Array(items)) if items.iter().all(Value::is_string) => items
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join("\n"),
        (TextFormat::Integer { .. } | TextFormat::Number, Value::Number(number)) => {
            number.to_string()
        }
        (TextFormat::Json | TextFormat::Lines | TextFormat::Multiline, _) => {
            serde_json::to_string_pretty(value).unwrap_or_default()
        }
        _ => serde_json::to_string(value).unwrap_or_default(),
    }
}

/// Parses control text into a config value. Blank text means "not set", so
/// the module falls back to its own default.
pub fn parse_text(format: TextFormat, text: &str) -> Result<Option<Value>, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    match format {
        TextFormat::Integer { minimum, maximum } => {
            let number = trimmed
                .parse::<i64>()
                .map_err(|_| "Нужно целое число".to_string())?;
            if let Some(minimum) = minimum.filter(|minimum| number < *minimum) {
                return Err(format!("Не меньше {minimum}"));
            }
            if let Some(maximum) = maximum.filter(|maximum| number > *maximum) {
                return Err(format!("Не больше {maximum}"));
            }
            Ok(Some(Value::from(number)))
        }
        TextFormat::Number => trimmed
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map(|number| Some(Value::Number(number)))
            .ok_or_else(|| "Нужно число".to_string()),
        TextFormat::Line | TextFormat::Secret | TextFormat::Multiline => {
            Ok(Some(Value::String(text.to_string())))
        }
        TextFormat::Lines => Ok(Some(Value::Array(
            text.lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(|line| Value::String(line.to_string()))
                .collect(),
        ))),
        TextFormat::Json => match serde_json::from_str::<Value>(trimmed) {
            Ok(Value::Null) => Ok(None),
            Ok(value) => Ok(Some(value)),
            Err(error) => Err(format!("Неверный JSON: {error}")),
        },
    }
}

/// Human-readable module default, shown when the value is not set.
pub fn default_hint(field: &ConfigField) -> Option<String> {
    let default = field.default.as_ref()?;
    let text = match field_kind(&field.value) {
        FieldKind::Switch => match default.as_bool() {
            Some(true) => "включено".to_string(),
            Some(false) => "выключено".to_string(),
            None => choice_key(default),
        },
        FieldKind::Choice(options) => choice_title(options, default)
            .map(str::to_string)
            .unwrap_or_else(|| choice_key(default)),
        FieldKind::MultiChoice(options) => match default.as_array() {
            Some(values) if values.is_empty() => "ничего".to_string(),
            Some(values) => values
                .iter()
                .map(|value| {
                    choice_title(options, value)
                        .map(str::to_string)
                        .unwrap_or_else(|| choice_key(value))
                })
                .collect::<Vec<_>>()
                .join(", "),
            None => choice_key(default),
        },
        FieldKind::Text(TextFormat::Lines) => match default.as_array() {
            Some(values) if values.is_empty() => "пусто".to_string(),
            _ => format_value(TextFormat::Lines, default).replace('\n', ", "),
        },
        FieldKind::Text(format) => format_value(format, default),
    };
    Some(text)
}

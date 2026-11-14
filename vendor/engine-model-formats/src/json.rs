//! Чтение полей JSON с понятными ошибками («element `body`: `from` must be 3 numbers»).

use serde_json::{Map, Value};

use crate::{limits, Error};

/// Разобрать JSON (BOM в начале допускается).
pub fn parse(bytes: &[u8], what: &str) -> Result<Value, Error> {
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    serde_json::from_slice(bytes).map_err(|e| Error(format!("not a valid {what}: {e}")))
}

/// Число как `f32`: конечное и не больше [`limits::MAX_COORDINATE`] по модулю.
pub fn number(value: &Value, context: &dyn Fn() -> String) -> Result<f32, Error> {
    let n = value.as_f64().ok_or_else(|| Error(format!("{} must be a number", context())))?;
    if !n.is_finite() || n.abs() > limits::MAX_COORDINATE as f64 {
        return Err(Error(format!("{} is out of range ({n})", context())));
    }
    Ok(n as f32)
}

/// Массив из `N` чисел; поле отсутствует — `default`.
pub fn numbers<const N: usize>(object: &Map<String, Value>, key: &str, default: Option<[f32; N]>, owner: &dyn Fn() -> String) -> Result<[f32; N], Error> {
    let Some(value) = object.get(key).filter(|v| !v.is_null()) else {
        return default.ok_or_else(|| Error(format!("{}: `{key}` is missing", owner())));
    };
    let list = value
        .as_array()
        .filter(|a| a.len() >= N)
        .ok_or_else(|| Error(format!("{}: `{key}` must be a list of {N} numbers", owner())))?;
    let mut out = [0.0; N];
    for (i, item) in list.iter().take(N).enumerate() {
        out[i] = number(item, &|| format!("{}: `{key}`[{i}]", owner()))?;
    }
    Ok(out)
}

pub fn string<'a>(object: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    object.get(key).and_then(Value::as_str)
}

pub fn boolean(object: &Map<String, Value>, key: &str, default: bool) -> bool {
    match object.get(key) {
        Some(Value::Bool(b)) => *b,
        // Старые файлы пишут флаги числами 0 / 1.
        Some(Value::Number(n)) => n.as_f64().is_some_and(|n| n != 0.0),
        _ => default,
    }
}

/// Строка для сообщений: не длиннее 60 символов.
pub fn short(text: &str) -> String {
    if text.chars().count() <= 60 {
        text.to_string()
    } else {
        format!("{}…", text.chars().take(59).collect::<String>())
    }
}

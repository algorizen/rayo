//! `rayo-schema` — compiles the flat schema IR produced by Python
//! introspection into validators and serializers. See ADR-0005, ADR-0006 and
//! docs/design/schema-compiler.md.
//!
//! v0 scope: [`write_json`], the Rust-direct response serializer. It walks the
//! object a handler returned and writes JSON bytes directly — it READS Python
//! objects but never creates one (hard invariant 3). The schema-IR-driven
//! serializer that replaces this generic walk lands with typed models (M1 W2).

#![forbid(unsafe_code)]

use pyo3::exceptions::{PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBool, PyDict, PyFloat, PyInt, PyList, PyString, PyTuple};

/// Serialize `value` as JSON onto `out`. Supported: dict (str keys), list,
/// tuple, str, int, float, bool, None — the JSON-shaped core. Typed models
/// arrive with the schema IR.
pub fn write_json(value: &Bound<'_, PyAny>, out: &mut Vec<u8>) -> PyResult<()> {
    if value.is_none() {
        out.extend_from_slice(b"null");
    } else if let Ok(boolean) = value.cast::<PyBool>() {
        // bool first: PyBool is a PyInt subclass and would serialize as 0/1.
        out.extend_from_slice(if boolean.is_true() { b"true" } else { b"false" });
    } else if value.cast::<PyInt>().is_ok() {
        let integer: i64 = value.extract().map_err(|_| {
            PyValueError::new_err(
                "Rayo cannot serialize integers outside the 64-bit range to JSON yet",
            )
        })?;
        out.extend_from_slice(integer.to_string().as_bytes());
    } else if value.cast::<PyFloat>().is_ok() {
        let number: f64 = value.extract()?;
        if !number.is_finite() {
            return Err(PyValueError::new_err(
                "JSON cannot represent NaN or infinity; return None or a string instead",
            ));
        }
        out.extend_from_slice(number.to_string().as_bytes());
    } else if let Ok(text) = value.cast::<PyString>() {
        write_json_string(text.to_str()?, out);
    } else if let Ok(mapping) = value.cast::<PyDict>() {
        out.push(b'{');
        let mut first_entry = true;
        for (entry_key, entry_value) in mapping.iter() {
            if !first_entry {
                out.push(b',');
            }
            first_entry = false;
            let Ok(key_text) = entry_key.cast::<PyString>() else {
                return Err(PyTypeError::new_err(format!(
                    "JSON object keys must be str, found {}",
                    entry_key.get_type().name()?
                )));
            };
            write_json_string(key_text.to_str()?, out);
            out.push(b':');
            write_json(&entry_value, out)?;
        }
        out.push(b'}');
    } else if let Ok(items) = value.cast::<PyList>() {
        write_json_sequence(items.iter(), out)?;
    } else if let Ok(items) = value.cast::<PyTuple>() {
        write_json_sequence(items.iter(), out)?;
    } else {
        return Err(PyTypeError::new_err(format!(
            "Rayo cannot serialize {} to JSON yet (v0 supports dict, list, tuple, \
             str, int, float, bool, None; typed models arrive with the schema engine)",
            value.get_type().name()?
        )));
    }
    Ok(())
}

fn write_json_sequence<'py>(
    items: impl Iterator<Item = Bound<'py, PyAny>>,
    out: &mut Vec<u8>,
) -> PyResult<()> {
    out.push(b'[');
    for (index, item) in items.enumerate() {
        if index > 0 {
            out.push(b',');
        }
        write_json(&item, out)?;
    }
    out.push(b']');
    Ok(())
}

/// JSON string escaping per RFC 8259: `"` `\` and control characters are
/// escaped; everything else passes through as UTF-8.
pub fn write_json_string(text: &str, out: &mut Vec<u8>) {
    out.push(b'"');
    for character in text.chars() {
        match character {
            '"' => out.extend_from_slice(b"\\\""),
            '\\' => out.extend_from_slice(b"\\\\"),
            '\n' => out.extend_from_slice(b"\\n"),
            '\r' => out.extend_from_slice(b"\\r"),
            '\t' => out.extend_from_slice(b"\\t"),
            control if (control as u32) < 0x20 => {
                let escaped = format!("\\u{:04x}", control as u32);
                out.extend_from_slice(escaped.as_bytes());
            }
            passthrough => {
                let mut utf8_buffer = [0u8; 4];
                out.extend_from_slice(passthrough.encode_utf8(&mut utf8_buffer).as_bytes());
            }
        }
    }
    out.push(b'"');
}

#[cfg(test)]
mod tests {
    use super::write_json_string;

    fn escaped(text: &str) -> String {
        let mut out = Vec::new();
        write_json_string(text, &mut out);
        String::from_utf8(out).unwrap_or_else(|invalid| panic!("non-UTF-8 output: {invalid}"))
    }

    #[test]
    fn escapes_quotes_backslashes_and_control_characters() {
        assert_eq!(escaped(r#"say "hi"\now"#), r#""say \"hi\"\\now""#);
        assert_eq!(escaped("line\nbreak\ttab"), r#""line\nbreak\ttab""#);
        assert_eq!(escaped("\u{1}"), r#""\u0001""#);
    }

    #[test]
    fn passes_unicode_through_as_utf8() {
        assert_eq!(escaped("rayo ⚡ señal"), "\"rayo ⚡ señal\"");
    }
}

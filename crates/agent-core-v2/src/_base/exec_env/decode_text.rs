use std::{error::Error, fmt};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextDecodeErrors {
    Strict,
    Replace,
    Ignore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextEncoding {
    Utf8,
    Utf16Le,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextDecodeError {
    encoding: TextEncoding,
}

impl fmt::Display for TextDecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid {:?} input", self.encoding)
    }
}

impl Error for TextDecodeError {}

// Original: packages/agent-core-v2/src/_base/execEnv/decodeText.ts,
// decodeTextWithErrors().
pub fn decode_text_with_errors(
    data: &[u8],
    encoding: TextEncoding,
    errors: TextDecodeErrors,
    ignore_bom: bool,
) -> Result<String, TextDecodeError> {
    let mut output = match (encoding, errors) {
        (TextEncoding::Utf8, TextDecodeErrors::Strict) => std::str::from_utf8(data)
            .map(str::to_owned)
            .map_err(|_| TextDecodeError { encoding })?,
        (TextEncoding::Utf8, TextDecodeErrors::Replace) => {
            String::from_utf8_lossy(data).into_owned()
        }
        (TextEncoding::Utf8, TextDecodeErrors::Ignore) => decode_utf8_ignore(data),
        (TextEncoding::Utf16Le, mode) => decode_utf16_le(data, mode)?,
    };
    if !ignore_bom && output.starts_with('\u{feff}') {
        output.remove(0);
    }
    Ok(output)
}

fn decode_utf8_ignore(data: &[u8]) -> String {
    let mut output = String::new();
    let mut position = 0;
    while position < data.len() {
        let width = utf8_sequence_width(data[position]);
        if width != 0
            && position + width <= data.len()
            && let Ok(value) = std::str::from_utf8(&data[position..position + width])
        {
            output.push_str(value);
            position += width;
            continue;
        }
        position += 1;
    }
    output
}

fn utf8_sequence_width(first: u8) -> usize {
    match first {
        0x00..=0x7f => 1,
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => 0,
    }
}

fn decode_utf16_le(data: &[u8], errors: TextDecodeErrors) -> Result<String, TextDecodeError> {
    if !data.len().is_multiple_of(2) && errors == TextDecodeErrors::Strict {
        return Err(TextDecodeError {
            encoding: TextEncoding::Utf16Le,
        });
    }
    let units = data
        .chunks_exact(2)
        .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]));
    let mut output = String::new();
    for decoded in char::decode_utf16(units) {
        match decoded {
            Ok(character) => output.push(character),
            Err(_) if errors == TextDecodeErrors::Strict => {
                return Err(TextDecodeError {
                    encoding: TextEncoding::Utf16Le,
                });
            }
            Err(_) if errors == TextDecodeErrors::Replace => {
                output.push(char::REPLACEMENT_CHARACTER)
            }
            Err(_) => {}
        }
    }
    if !data.len().is_multiple_of(2) && errors == TextDecodeErrors::Replace {
        output.push(char::REPLACEMENT_CHARACTER);
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_strict_replace_and_ignore_preserve_source_error_modes() {
        let bytes = b"a\xffb\xe2\x82\xac";
        assert!(
            decode_text_with_errors(bytes, TextEncoding::Utf8, TextDecodeErrors::Strict, false)
                .is_err()
        );
        assert_eq!(
            decode_text_with_errors(bytes, TextEncoding::Utf8, TextDecodeErrors::Replace, false)
                .unwrap(),
            "a�b€"
        );
        assert_eq!(
            decode_text_with_errors(bytes, TextEncoding::Utf8, TextDecodeErrors::Ignore, false)
                .unwrap(),
            "ab€"
        );
    }

    #[test]
    fn utf16_handles_surrogates_odd_bytes_and_bom_policy() {
        let valid = [0xff, 0xfe, 0x3d, 0xd8, 0x00, 0xde];
        assert_eq!(
            decode_text_with_errors(
                &valid,
                TextEncoding::Utf16Le,
                TextDecodeErrors::Strict,
                false
            )
            .unwrap(),
            "😀"
        );
        assert!(
            decode_text_with_errors(
                &[0x00],
                TextEncoding::Utf16Le,
                TextDecodeErrors::Strict,
                false
            )
            .is_err()
        );
        assert_eq!(
            decode_text_with_errors(
                &valid,
                TextEncoding::Utf16Le,
                TextDecodeErrors::Strict,
                true
            )
            .unwrap(),
            "\u{feff}😀"
        );
    }
}

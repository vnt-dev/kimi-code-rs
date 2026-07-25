// Original: requestLogging.ts, extractEnvelopeCode().
pub fn extract_envelope_code(payload: Option<&str>) -> Option<i64> {
    let input = payload?.trim_start();
    let mut rest = input.strip_prefix('{')?.trim_start();
    rest = rest.strip_prefix('"')?;
    rest = rest.strip_prefix("code")?;
    rest = rest.strip_prefix('"')?.trim_start();
    rest = rest.strip_prefix(':')?.trim_start();

    let (negative, digits) = match rest.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, rest),
    };
    let length = digits.bytes().take_while(u8::is_ascii_digit).count();
    if length == 0 {
        return None;
    }
    let value = digits[..length].parse::<i64>().ok()?;
    Some(if negative { -value } else { value })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_only_a_leading_integer_envelope_code() {
        assert_eq!(
            extract_envelope_code(Some("{\"code\":0,\"msg\":\"success\",\"data\":null}")),
            Some(0)
        );
        assert_eq!(
            extract_envelope_code(Some(" \n { \"code\" : -40001, broken")),
            Some(-40001)
        );
        assert_eq!(extract_envelope_code(Some("{\"msg\":\"no code\"}")), None);
        assert_eq!(extract_envelope_code(Some("<html/>")), None);
        assert_eq!(extract_envelope_code(None), None);
    }
}

/// Escapes XML text content (`&`, `<`, and `>`).
pub fn escape_xml_text(input: &str) -> String {
    quick_xml::escape::partial_escape(input).into_owned()
}

/// Escapes an XML attribute value, including both quote styles.
pub fn escape_xml_attribute(input: &str) -> String {
    quick_xml::escape::escape(input).into_owned()
}

// Original:
//   packages/agent-core-v2/src/_base/utils/xml-escape.ts
//   escapeXmlTags()
// This is tag neutralization for prompt templates, not XML serialization.
pub fn escape_xml_tags(input: &str) -> String {
    input.replace('<', "&lt;").replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_escape_uses_xml_text_rules() {
        assert_eq!(
            escape_xml_text(r#"<&>"' &amp;"#),
            "&lt;&amp;&gt;\"' &amp;amp;"
        );
    }

    #[test]
    fn attribute_escape_uses_complete_xml_rules() {
        assert_eq!(
            escape_xml_attribute(r#"<&>"'"#),
            "&lt;&amp;&gt;&quot;&apos;"
        );
    }

    #[test]
    fn tag_escape_only_handles_angle_brackets() {
        assert_eq!(escape_xml_tags(r#"<&>"'"#), r#"&lt;&&gt;"'"#);
    }

    #[test]
    fn empty_and_plain_text_are_unchanged() {
        for value in ["", "plain text", "中文"] {
            assert_eq!(escape_xml_text(value), value);
            assert_eq!(escape_xml_attribute(value), value);
            assert_eq!(escape_xml_tags(value), value);
        }
    }
}

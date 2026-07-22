// Original:
//   packages/agent-core-v2/src/_base/utils/xml-escape.ts
//   escapeXml()
pub fn escape_xml(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

// Original:
//   packages/agent-core-v2/src/_base/utils/xml-escape.ts
//   escapeXmlAttr()
pub fn escape_xml_attr(input: &str) -> String {
    input.replace('&', "&amp;").replace('"', "&quot;")
}

// Original:
//   packages/agent-core-v2/src/_base/utils/xml-escape.ts
//   escapeXmlTags()
pub fn escape_xml_tags(input: &str) -> String {
    input.replace('<', "&lt;").replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_escape_preserves_source_character_set_and_order() {
        assert_eq!(
            escape_xml(r#"<&>"' &amp;"#),
            "&lt;&amp;&gt;&quot;' &amp;amp;"
        );
    }

    #[test]
    fn attribute_escape_only_handles_ampersand_and_double_quote() {
        assert_eq!(escape_xml_attr(r#"<&>"'"#), "<&amp;>&quot;'");
    }

    #[test]
    fn tag_escape_only_handles_angle_brackets() {
        assert_eq!(escape_xml_tags(r#"<&>"'"#), r#"&lt;&&gt;"'"#);
    }

    #[test]
    fn empty_and_plain_text_are_unchanged() {
        for value in ["", "plain text", "中文"] {
            assert_eq!(escape_xml(value), value);
            assert_eq!(escape_xml_attr(value), value);
            assert_eq!(escape_xml_tags(value), value);
        }
    }
}

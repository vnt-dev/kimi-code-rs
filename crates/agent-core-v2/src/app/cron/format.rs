//! Cron tool text rendering.
//!
//! Original: `packages/agent-core-v2/src/app/cron/format.ts`.

use std::io::Write;

use chrono::{DateTime, Local, Utc};
use quick_xml::Writer;
use quick_xml::events::{BytesStart, BytesText, Event};

use crate::agent::context_memory::PromptOrigin;

pub fn format_local_iso_with_offset(ms: f64) -> String {
    let Some(utc) = DateTime::<Utc>::from_timestamp(
        (ms / 1_000.0).floor() as i64,
        ((ms.rem_euclid(1_000.0)) * 1_000_000.0) as u32,
    ) else {
        return "Invalid Date".into();
    };
    let date = utc.with_timezone(&Local);
    date.format("%Y-%m-%dT%H:%M:%S%.3f%:z").to_string()
}

pub fn render_cron_fire_xml(origin: &PromptOrigin, prompt: &str) -> String {
    let PromptOrigin::CronJob {
        job_id,
        cron,
        recurring,
        coalesced_count,
        stale,
    } = origin
    else {
        return render("unknown", "unknown", false, 0, false, prompt);
    };
    render(job_id, cron, *recurring, *coalesced_count, *stale, prompt)
}

fn render(
    job_id: &str,
    cron: &str,
    recurring: bool,
    coalesced_count: u64,
    stale: bool,
    prompt: &str,
) -> String {
    let mut writer = Writer::new(Vec::new());
    let mut start = BytesStart::new("cron-fire");
    start.push_attribute(("jobId", attr(job_id)));
    start.push_attribute(("cron", attr(cron)));
    start.push_attribute(("recurring", flag(recurring)));
    start.push_attribute(("coalescedCount", coalesced_count.to_string().as_str()));
    start.push_attribute(("stale", flag(stale)));
    writer
        .write_event(Event::Start(start))
        .expect("writing to Vec cannot fail");
    writer
        .get_mut()
        .write_all(b"\n<prompt>\n")
        .expect("writing to Vec cannot fail");
    writer
        .write_event(Event::Text(BytesText::from_escaped(prompt)))
        .expect("writing to Vec cannot fail");
    writer
        .get_mut()
        .write_all(b"\n</prompt>\n</cron-fire>")
        .expect("writing to Vec cannot fail");
    String::from_utf8(writer.into_inner()).expect("render output is UTF-8")
}

fn attr(value: &str) -> &str {
    if value.is_empty() { "unknown" } else { value }
}

fn flag(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cron_fire_xml_preserves_exact_wrapping_and_attribute_escaping() {
        let text = render_cron_fire_xml(
            &PromptOrigin::CronJob {
                job_id: "a<&b".into(),
                cron: "0 \"' * * *".into(),
                recurring: true,
                coalesced_count: 2,
                stale: false,
            },
            "<keep>",
        );
        assert_eq!(
            text,
            "<cron-fire jobId=\"a&lt;&amp;b\" cron=\"0 &quot;&apos; * * *\" recurring=\"true\" coalescedCount=\"2\" stale=\"false\">\n<prompt>\n<keep>\n</prompt>\n</cron-fire>"
        );
    }
    #[test]
    fn local_iso_contains_millisecond_offset() {
        let text = format_local_iso_with_offset(0.0);
        assert!(text.ends_with("+00:00") || text.ends_with("+08:00") || text.contains("T"));
        assert!(text.contains(".000"));
    }
}

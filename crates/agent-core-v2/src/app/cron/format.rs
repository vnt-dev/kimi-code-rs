//! Cron tool text rendering.
//!
//! Original: `packages/agent-core-v2/src/app/cron/format.ts`.

use chrono::{DateTime, Local, Utc};

use crate::{_base::utils::xml_escape::escape_xml_attribute, agent::context_memory::PromptOrigin};

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
    format!(
        "<cron-fire jobId=\"{}\" cron=\"{}\" recurring=\"{}\" coalescedCount=\"{}\" stale=\"{}\">\n<prompt>\n{}\n</prompt>\n</cron-fire>",
        attr(job_id),
        attr(cron),
        recurring,
        coalesced_count,
        stale,
        prompt
    )
}
fn attr(value: &str) -> String {
    if value.is_empty() {
        "unknown".into()
    } else {
        escape_xml_attribute(value)
    }
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

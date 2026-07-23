use std::{
    collections::{HashMap, HashSet},
    panic::{AssertUnwindSafe, catch_unwind},
    sync::Arc,
    time::Instant,
};

use futures_util::future::join_all;
use regress::Regex;
use serde_json::{Map, Value};

use crate::{
    agent::external_hooks::{
        HookAction, HookBlockDecision, HookDef, HookMatcherValue, HookResult, RunHookOptions,
        run_hook,
    },
    kosong::contract::message::ContentPart,
    os::interface::host_process::HostProcessService,
};

use super::contract::ExternalHooksRunnerTriggerArgs;

const DEFAULT_HOOK_TIMEOUT_SECONDS: f64 = 30.0;

pub type HooksByEvent = HashMap<String, Vec<HookDef>>;
pub type HookTriggeredCallback = Arc<dyn Fn(&str, &str, usize) + Send + Sync>;
pub type HookResolvedCallback = Arc<dyn Fn(&str, &str, &str, Option<&str>, u64) + Send + Sync>;

#[derive(Clone, Default)]
pub struct HookRunCallbacks {
    pub on_triggered: Option<HookTriggeredCallback>,
    pub on_resolved: Option<HookResolvedCallback>,
}

// Original:
//   packages/agent-core-v2/src/app/externalHooksRunner/runner.ts
//   indexHooks()
pub fn index_hooks(hooks: &[HookDef]) -> HooksByEvent {
    let mut by_event = HashMap::new();
    for hook in hooks {
        by_event
            .entry(hook.event.as_str().to_owned())
            .or_insert_with(Vec::new)
            .push(hook.clone());
    }
    by_event
}

// Original: runner.ts, runMatchedHooks(). Hook processes intentionally run
// concurrently because the source uses Promise.all and preserves input order.
pub async fn run_matched_hooks(
    host_process: &dyn HostProcessService,
    by_event: &HooksByEvent,
    event: &str,
    args: &ExternalHooksRunnerTriggerArgs,
    callbacks: &HookRunCallbacks,
) -> Vec<HookResult> {
    let matcher_value = matcher_value_text(args.matcher_value.as_ref());
    let cwd = args.cwd.as_deref().unwrap_or_default();
    let mut matched = Vec::new();
    let mut seen = HashSet::new();
    for hook in by_event.get(event).map(Vec::as_slice).unwrap_or_default() {
        if !matches_pattern(hook.matcher.as_deref().unwrap_or_default(), &matcher_value) {
            continue;
        }
        let key = format!(
            "{}\0{}",
            hook.cwd.as_deref().unwrap_or_default(),
            hook.command
        );
        if seen.insert(key) {
            matched.push(hook);
        }
    }
    if matched.is_empty() {
        return Vec::new();
    }

    if let Some(callback) = &callbacks.on_triggered {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            callback(event, &matcher_value, matched.len());
        }));
    }

    let mut input = Map::from_iter([
        ("hookEventName".into(), Value::String(event.into())),
        (
            "sessionId".into(),
            Value::String(args.session_id.clone().unwrap_or_default()),
        ),
        ("cwd".into(), Value::String(cwd.into())),
    ]);
    if let Some(input_data) = &args.input_data {
        input.extend(input_data.clone());
    }
    let input = to_hook_input_data(input);
    let started_at = Instant::now();
    let results = join_all(matched.into_iter().map(|hook| {
        run_hook(
            host_process,
            &hook.command,
            &input,
            RunHookOptions {
                timeout: hook
                    .timeout
                    .map(|timeout| timeout as f64)
                    .unwrap_or(DEFAULT_HOOK_TIMEOUT_SECONDS),
                cwd: hook
                    .cwd
                    .clone()
                    .or_else(|| (!cwd.is_empty()).then(|| cwd.into())),
                env: hook.env.clone(),
                signal: args.signal.clone(),
            },
        )
    }))
    .await;

    let decision = block_decision(event, &results);
    if let Some(callback) = &callbacks.on_resolved {
        let action = if decision.is_some() { "block" } else { "allow" };
        let reason = decision.as_ref().map(|decision| decision.reason.as_str());
        let elapsed = u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX);
        let _ = catch_unwind(AssertUnwindSafe(|| {
            callback(event, &matcher_value, action, reason, elapsed);
        }));
    }
    results
}

// Original: runner.ts, blockDecision().
pub fn block_decision(event: &str, results: &[HookResult]) -> Option<HookBlockDecision> {
    let block = results
        .iter()
        .find(|result| result.action == HookAction::Block)?;
    let reason = block
        .reason
        .as_deref()
        .map(trim_ecmascript)
        .filter(|reason| !reason.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("Blocked by {event} hook"));
    Some(HookBlockDecision::new(reason))
}

// Original: runner.ts, matches(). `new RegExp(pattern)` has no Unicode flag,
// so values are matched as JavaScript UCS-2 code units.
fn matches_pattern(pattern: &str, value: &str) -> bool {
    if pattern.is_empty() {
        return true;
    }
    let Ok(regex) = Regex::new(pattern) else {
        return false;
    };
    let value: Vec<u16> = value.encode_utf16().collect();
    regex.find_from_ucs2(&value, 0).next().is_some()
}

// Original: runner.ts, matcherValueText().
fn matcher_value_text(value: Option<&HookMatcherValue>) -> String {
    match value {
        None => String::new(),
        Some(HookMatcherValue::String(value)) => value.clone(),
        Some(HookMatcherValue::Content(parts)) => parts
            .iter()
            .filter_map(|part| match part {
                ContentPart::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" "),
    }
}

// Original: runner.ts, toHookInputData().
fn to_hook_input_data(input: Map<String, Value>) -> Map<String, Value> {
    input
        .into_iter()
        .map(|(key, value)| (camel_to_snake(&key), value))
        .collect()
}

// Original: runner.ts, camelToSnake(). Only ASCII A-Z are replaced by the
// source regular expression; existing underscores and leading capitals stay.
fn camel_to_snake(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for character in value.chars() {
        if character.is_ascii_uppercase() {
            output.push('_');
            output.push(character.to_ascii_lowercase());
        } else {
            output.push(character);
        }
    }
    output
}

fn trim_ecmascript(value: &str) -> &str {
    value.trim_matches(|character: char| character.is_whitespace() || character == '\u{feff}')
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::kosong::contract::message::MediaUrl;

    fn hook(event: crate::agent::external_hooks::HookEventType, command: &str) -> HookDef {
        HookDef {
            event,
            matcher: None,
            command: command.into(),
            timeout: None,
            cwd: None,
            env: None,
        }
    }

    fn result(action: HookAction, reason: Option<&str>) -> HookResult {
        HookResult {
            action,
            message: None,
            reason: reason.map(str::to_owned),
            stdout: None,
            stderr: None,
            exit_code: None,
            timed_out: None,
            structured_output: None,
        }
    }

    #[test]
    fn indexes_in_source_order_and_keeps_events_separate() {
        use crate::agent::external_hooks::HookEventType::{PreToolUse, Stop};
        let indexed = index_hooks(&[
            hook(Stop, "one"),
            hook(PreToolUse, "two"),
            hook(Stop, "three"),
        ]);
        assert_eq!(
            indexed["Stop"]
                .iter()
                .map(|hook| hook.command.as_str())
                .collect::<Vec<_>>(),
            ["one", "three"]
        );
        assert_eq!(indexed["PreToolUse"][0].command, "two");
    }

    #[test]
    fn matcher_uses_ecmascript_backreferences_lookahead_and_ucs2() {
        assert!(matches_pattern(r"(\w)\1", "book"));
        assert!(matches_pattern(r"foo(?=bar)", "foobar"));
        assert!(!matches_pattern("[invalid", "anything"));
        assert!(matches_pattern("", "anything"));
        assert!(!matches_pattern(r"^.$", "😀"));
    }

    #[test]
    fn content_matcher_uses_only_text_parts_with_spaces() {
        let value = HookMatcherValue::Content(vec![
            ContentPart::Text {
                text: "hello".into(),
            },
            ContentPart::ImageUrl {
                image_url: MediaUrl {
                    url: "file:///a.png".into(),
                    id: None,
                },
            },
            ContentPart::Think {
                think: "hidden".into(),
                encrypted: None,
            },
            ContentPart::Text {
                text: "world".into(),
            },
        ]);
        assert_eq!(matcher_value_text(Some(&value)), "hello world");
        assert_eq!(matcher_value_text(None), "");
    }

    #[test]
    fn input_keys_are_shallowly_converted_like_ascii_regex_replacement() {
        let input = Map::from_iter([
            ("toolName".into(), Value::String("Bash".into())),
            ("URLValue".into(), Value::String("x".into())),
            ("nested".into(), serde_json::json!({"innerKey": true})),
        ]);
        let output = to_hook_input_data(input);
        assert_eq!(output["tool_name"], "Bash");
        assert_eq!(output["_u_r_l_value"], "x");
        assert_eq!(output["nested"]["innerKey"], true);
    }

    #[test]
    fn block_decision_uses_first_block_reason_or_default() {
        assert_eq!(
            block_decision(
                "PreToolUse",
                &[
                    result(HookAction::Allow, None),
                    result(HookAction::Block, Some("\u{feff} denied "))
                ],
            )
            .unwrap(),
            HookBlockDecision::new("denied")
        );
        assert_eq!(
            block_decision("Stop", &[result(HookAction::Block, Some("  "))])
                .unwrap()
                .reason,
            "Blocked by Stop hook"
        );
        assert!(block_decision("Stop", &[result(HookAction::Allow, None)]).is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn runs_deduped_matches_with_merged_input_and_callbacks() {
        use crate::agent::external_hooks::HookEventType::PreToolUse;

        let hooks = index_hooks(&[hook(PreToolUse, "cat"), hook(PreToolUse, "cat")]);
        let triggered = Arc::new(Mutex::new(Vec::new()));
        let resolved = Arc::new(Mutex::new(Vec::new()));
        let callbacks = HookRunCallbacks {
            on_triggered: Some({
                let triggered = Arc::clone(&triggered);
                Arc::new(move |event, target, count| {
                    triggered
                        .lock()
                        .unwrap()
                        .push((event.to_owned(), target.to_owned(), count));
                })
            }),
            on_resolved: Some({
                let resolved = Arc::clone(&resolved);
                Arc::new(move |event, target, action, reason, _duration| {
                    resolved.lock().unwrap().push((
                        event.to_owned(),
                        target.to_owned(),
                        action.to_owned(),
                        reason.map(str::to_owned),
                    ));
                })
            }),
        };
        let args = ExternalHooksRunnerTriggerArgs {
            matcher_value: Some(HookMatcherValue::String("Bash".into())),
            input_data: Some(Map::from_iter([
                ("toolName".into(), Value::String("Bash".into())),
                ("cwd".into(), Value::String("caller-override".into())),
            ])),
            cwd: Some("/tmp".into()),
            session_id: Some("ses_1".into()),
            ..ExternalHooksRunnerTriggerArgs::default()
        };
        let results = run_matched_hooks(
            &crate::os::backends::node_local::host_process_service::LocalHostProcessService::default(),
            &hooks,
            "PreToolUse",
            &args,
            &callbacks,
        )
        .await;

        assert_eq!(results.len(), 1);
        let input: Value = serde_json::from_str(results[0].stdout.as_deref().unwrap()).unwrap();
        assert_eq!(input["hook_event_name"], "PreToolUse");
        assert_eq!(input["session_id"], "ses_1");
        assert_eq!(input["cwd"], "caller-override");
        assert_eq!(input["tool_name"], "Bash");
        assert_eq!(
            *triggered.lock().unwrap(),
            [("PreToolUse".into(), "Bash".into(), 1)]
        );
        assert_eq!(
            *resolved.lock().unwrap(),
            [("PreToolUse".into(), "Bash".into(), "allow".into(), None)]
        );
    }
}

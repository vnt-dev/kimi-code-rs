use serde::Serialize;
use serde_json::Value;

use super::options::PromptOutputFormat;

pub trait PromptOutput {
    fn columns(&self) -> Option<usize> {
        None
    }

    fn write(&mut self, chunk: &str) -> bool;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookResultEvent {
    pub hook_event: String,
    pub content: String,
    pub blocked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryingEvent {
    pub failed_attempt: u64,
    pub next_attempt: u64,
    pub max_attempts: u64,
    pub delay_ms: u64,
    pub error_name: String,
    pub error_message: String,
    pub status_code: Option<u16>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PromptValue {
    String(String),
    Json(Value),
    Undefined,
}

impl From<Value> for PromptValue {
    fn from(value: Value) -> Self {
        match value {
            Value::String(value) => Self::String(value),
            value => Self::Json(value),
        }
    }
}

pub trait PromptTurnWriter {
    fn write_assistant_delta(&mut self, delta: &str);
    fn write_hook_result(&mut self, event: &HookResultEvent);
    fn write_thinking_delta(&mut self, delta: &str);
    fn write_tool_call(&mut self, tool_call_id: &str, name: &str, args: &PromptValue);
    fn write_tool_call_delta(
        &mut self,
        tool_call_id: &str,
        name: Option<&str>,
        arguments_part: Option<&str>,
    );
    fn write_tool_result(&mut self, tool_call_id: &str, output: &PromptValue);
    fn write_retrying(&mut self, event: &RetryingEvent);
    fn flush_assistant(&mut self);
    fn discard_assistant(&mut self);
    fn finish(&mut self);
}

const PROMPT_BLOCK_BULLET: &str = "• ";
const PROMPT_BLOCK_INDENT: &str = "  ";

// Original:
//   apps/kimi-code/src/cli/prompt-render.ts
//   PromptTranscriptWriter
pub struct PromptTranscriptWriter<'a> {
    assistant_writer: PromptBlockWriter<'a>,
    thinking_writer: PromptBlockWriter<'a>,
}

impl<'a> PromptTranscriptWriter<'a> {
    pub fn new(stdout: &'a mut dyn PromptOutput, stderr: &'a mut dyn PromptOutput) -> Self {
        Self {
            assistant_writer: PromptBlockWriter::new(stdout),
            thinking_writer: PromptBlockWriter::new(stderr),
        }
    }

    /// Write tool progress exactly as the original driver writes to stderr,
    /// without changing the transcript block's indentation state.
    pub fn write_raw_stderr(&mut self, text: &str) {
        self.thinking_writer.write_raw(text);
    }
}

impl PromptTurnWriter for PromptTranscriptWriter<'_> {
    fn write_assistant_delta(&mut self, delta: &str) {
        self.thinking_writer.finish();
        self.assistant_writer.write(delta);
    }

    fn write_hook_result(&mut self, event: &HookResultEvent) {
        self.thinking_writer.finish();
        self.assistant_writer.finish();
        self.assistant_writer
            .write(&format_hook_result_plain(event));
        self.assistant_writer.finish();
    }

    fn write_thinking_delta(&mut self, delta: &str) {
        self.thinking_writer.write(delta);
    }

    fn write_tool_call(&mut self, _: &str, _: &str, _: &PromptValue) {}

    fn write_tool_call_delta(&mut self, _: &str, _: Option<&str>, _: Option<&str>) {}

    fn write_tool_result(&mut self, _: &str, _: &PromptValue) {}

    fn write_retrying(&mut self, _: &RetryingEvent) {}

    fn flush_assistant(&mut self) {
        self.assistant_writer.finish();
    }

    fn discard_assistant(&mut self) {}

    fn finish(&mut self) {
        self.thinking_writer.finish();
        self.assistant_writer.finish();
    }
}

#[derive(Debug, Clone, Serialize)]
struct JsonFunction {
    name: String,
    arguments: String,
}

#[derive(Debug, Clone, Serialize)]
struct JsonToolCall {
    #[serde(rename = "type")]
    kind: &'static str,
    id: String,
    function: JsonFunction,
}

#[derive(Serialize)]
struct JsonAssistantMessage<'a> {
    role: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<&'a [JsonToolCall]>,
}

#[derive(Serialize)]
struct JsonToolMessage<'a> {
    role: &'static str,
    tool_call_id: &'a str,
    content: String,
}

#[derive(Serialize)]
struct JsonRetryMessage<'a> {
    role: &'static str,
    #[serde(rename = "type")]
    kind: &'static str,
    failed_attempt: u64,
    next_attempt: u64,
    max_attempts: u64,
    delay_ms: u64,
    error_name: &'a str,
    error_message: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    status_code: Option<u16>,
}

// Original: PromptJsonWriter
pub struct PromptJsonWriter<'a> {
    stdout: &'a mut dyn PromptOutput,
    assistant_text: String,
    tool_calls: Vec<JsonToolCall>,
}

impl<'a> PromptJsonWriter<'a> {
    pub fn new(stdout: &'a mut dyn PromptOutput) -> Self {
        Self {
            stdout,
            assistant_text: String::new(),
            tool_calls: Vec::new(),
        }
    }

    fn find_or_create_tool_call(&mut self, tool_call_id: &str, name: &str) -> &mut JsonToolCall {
        if let Some(index) = self
            .tool_calls
            .iter()
            .position(|tool_call| tool_call.id == tool_call_id)
        {
            return &mut self.tool_calls[index];
        }
        self.tool_calls.push(JsonToolCall {
            kind: "function",
            id: tool_call_id.to_owned(),
            function: JsonFunction {
                name: name.to_owned(),
                arguments: String::new(),
            },
        });
        self.tool_calls.last_mut().expect("tool call was inserted")
    }

    fn write_json_line<T: Serialize>(&mut self, message: &T) {
        let json = serde_json::to_string(message).expect("prompt messages are serializable");
        self.stdout.write(&(json + "\n"));
    }
}

impl PromptTurnWriter for PromptJsonWriter<'_> {
    fn write_assistant_delta(&mut self, delta: &str) {
        self.assistant_text.push_str(delta);
    }

    fn write_hook_result(&mut self, event: &HookResultEvent) {
        self.flush_assistant();
        let content = format_hook_result_plain(event);
        self.write_json_line(&JsonAssistantMessage {
            role: "assistant",
            content: Some(&content),
            tool_calls: None,
        });
    }

    fn write_thinking_delta(&mut self, _: &str) {}

    fn write_tool_call(&mut self, tool_call_id: &str, name: &str, args: &PromptValue) {
        let arguments = stringify_json_value(args);
        let tool_call = self.find_or_create_tool_call(tool_call_id, name);
        tool_call.function.name = name.to_owned();
        tool_call.function.arguments = arguments;
    }

    fn write_tool_call_delta(
        &mut self,
        tool_call_id: &str,
        name: Option<&str>,
        arguments_part: Option<&str>,
    ) {
        let tool_call = self.find_or_create_tool_call(tool_call_id, name.unwrap_or_default());
        if let Some(name) = name {
            tool_call.function.name = name.to_owned();
        }
        if let Some(arguments_part) = arguments_part {
            tool_call.function.arguments.push_str(arguments_part);
        }
    }

    fn write_tool_result(&mut self, tool_call_id: &str, output: &PromptValue) {
        self.flush_assistant();
        self.write_json_line(&JsonToolMessage {
            role: "tool",
            tool_call_id,
            content: stringify_tool_output(output),
        });
    }

    fn write_retrying(&mut self, event: &RetryingEvent) {
        self.write_json_line(&JsonRetryMessage {
            role: "meta",
            kind: "turn.step.retrying",
            failed_attempt: event.failed_attempt,
            next_attempt: event.next_attempt,
            max_attempts: event.max_attempts,
            delay_ms: event.delay_ms,
            error_name: &event.error_name,
            error_message: &event.error_message,
            status_code: event.status_code,
        });
    }

    fn flush_assistant(&mut self) {
        if self.assistant_text.is_empty() && self.tool_calls.is_empty() {
            return;
        }
        let json = serde_json::to_string(&JsonAssistantMessage {
            role: "assistant",
            content: (!self.assistant_text.is_empty()).then_some(self.assistant_text.as_str()),
            tool_calls: (!self.tool_calls.is_empty()).then_some(self.tool_calls.as_slice()),
        })
        .expect("assistant message is serializable");
        self.stdout.write(&(json + "\n"));
        self.discard_assistant();
    }

    fn discard_assistant(&mut self) {
        self.assistant_text.clear();
        self.tool_calls.clear();
    }

    fn finish(&mut self) {
        self.flush_assistant();
    }
}

struct PromptBlockWriter<'a> {
    output: &'a mut dyn PromptOutput,
    started: bool,
    at_line_start: bool,
    line_width: usize,
    wrap_width: Option<usize>,
}

impl<'a> PromptBlockWriter<'a> {
    fn new(output: &'a mut dyn PromptOutput) -> Self {
        let wrap_width = output
            .columns()
            .filter(|columns| *columns > PROMPT_BLOCK_INDENT.len() + 1);
        Self {
            output,
            started: false,
            at_line_start: false,
            line_width: 0,
            wrap_width,
        }
    }

    fn write(&mut self, chunk: &str) {
        if chunk.is_empty() {
            return;
        }
        let mut rendered = self.start();
        for character in chunk.chars() {
            if self.at_line_start && character != '\n' {
                rendered.push_str(PROMPT_BLOCK_INDENT);
                self.at_line_start = false;
                self.line_width = PROMPT_BLOCK_INDENT.len();
            }
            let character_width = visible_char_width(character);
            if self.wrap_width.is_some_and(|width| {
                !self.at_line_start
                    && character != '\n'
                    && self.line_width + character_width > width
            }) {
                rendered.push('\n');
                rendered.push_str(PROMPT_BLOCK_INDENT);
                self.line_width = PROMPT_BLOCK_INDENT.len();
            }
            rendered.push(character);
            if character == '\n' {
                self.at_line_start = true;
                self.line_width = 0;
            } else {
                self.line_width += character_width;
            }
        }
        self.output.write(&rendered);
    }

    fn write_raw(&mut self, chunk: &str) {
        self.output.write(chunk);
    }

    fn finish(&mut self) {
        if !self.started {
            return;
        }
        self.output
            .write(if self.at_line_start { "\n" } else { "\n\n" });
        self.started = false;
        self.at_line_start = false;
        self.line_width = 0;
    }

    fn start(&mut self) -> String {
        if self.started {
            return String::new();
        }
        self.started = true;
        self.at_line_start = false;
        self.line_width = PROMPT_BLOCK_BULLET.chars().count();
        PROMPT_BLOCK_BULLET.to_owned()
    }
}

fn visible_char_width(character: char) -> usize {
    if character == '\t' { 4 } else { 1 }
}

fn format_hook_result_plain(event: &HookResultEvent) -> String {
    let blocked = if event.blocked { " blocked" } else { "" };
    let content = event.content.trim();
    format!(
        "{} hook{blocked}\n\n{}",
        event.hook_event,
        if content.is_empty() {
            "(empty)"
        } else {
            content
        }
    )
}

fn stringify_json_value(value: &PromptValue) -> String {
    match value {
        PromptValue::String(value) => value.clone(),
        PromptValue::Json(value) => serde_json::to_string(value).expect("JSON value"),
        PromptValue::Undefined => String::new(),
    }
}

fn stringify_tool_output(value: &PromptValue) -> String {
    match value {
        PromptValue::String(value) => value.clone(),
        PromptValue::Json(value) => serde_json::to_string(value).expect("JSON value"),
        PromptValue::Undefined => "undefined".to_owned(),
    }
}

#[derive(Serialize)]
struct VersionMessage<'a> {
    role: &'static str,
    #[serde(rename = "type")]
    kind: &'static str,
    version: &'a str,
}

pub fn write_experimental_version(
    version: &str,
    output_format: PromptOutputFormat,
    stdout: &mut dyn PromptOutput,
    stderr: &mut dyn PromptOutput,
) {
    if output_format == PromptOutputFormat::StreamJson {
        let line = serde_json::to_string(&VersionMessage {
            role: "meta",
            kind: "system.version",
            version,
        })
        .expect("version message is serializable");
        stdout.write(&(line + "\n"));
    } else {
        stderr.write(&format!("kimi version {version}\n"));
    }
}

#[derive(Serialize)]
struct ResumeMessage<'a> {
    role: &'static str,
    #[serde(rename = "type")]
    kind: &'static str,
    session_id: &'a str,
    command: &'a str,
    content: &'a str,
}

pub fn write_resume_hint(
    session_id: &str,
    output_format: PromptOutputFormat,
    stdout: &mut dyn PromptOutput,
    stderr: &mut dyn PromptOutput,
) {
    let command = format!("kimi -r {session_id}");
    let content = format!("To resume this session: {command}");
    if output_format == PromptOutputFormat::StreamJson {
        let line = serde_json::to_string(&ResumeMessage {
            role: "meta",
            kind: "session.resume_hint",
            session_id,
            command: &command,
            content: &content,
        })
        .expect("resume message is serializable");
        stdout.write(&(line + "\n"));
    } else {
        stderr.write(&(content + "\n"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Capture {
        text: String,
        columns: Option<usize>,
    }

    impl PromptOutput for Capture {
        fn columns(&self) -> Option<usize> {
            self.columns
        }

        fn write(&mut self, chunk: &str) -> bool {
            self.text.push_str(chunk);
            true
        }
    }

    #[test]
    fn renders_transcript_blocks_and_finishes_thinking_first() {
        let mut stdout = Capture::default();
        let mut stderr = Capture::default();
        {
            let mut writer = PromptTranscriptWriter::new(&mut stdout, &mut stderr);
            writer.write_thinking_delta("considering");
            writer.write_assistant_delta("hello\nworld");
            writer.finish();
        }
        assert_eq!(stderr.text, "• considering\n\n");
        assert_eq!(stdout.text, "• hello\n  world\n\n");
    }

    #[test]
    fn wraps_text_using_the_original_visible_width_rules() {
        let mut stdout = Capture {
            columns: Some(6),
            ..Capture::default()
        };
        let mut stderr = Capture::default();
        {
            let mut writer = PromptTranscriptWriter::new(&mut stdout, &mut stderr);
            writer.write_assistant_delta("abcde");
            writer.finish();
        }
        assert_eq!(stdout.text, "• abcd\n  e\n\n");
    }

    #[test]
    fn renders_hook_results_as_standalone_blocks() {
        let mut stdout = Capture::default();
        let mut stderr = Capture::default();
        {
            let mut writer = PromptTranscriptWriter::new(&mut stdout, &mut stderr);
            writer.write_hook_result(&HookResultEvent {
                hook_event: "before_prompt".to_owned(),
                content: "  ".to_owned(),
                blocked: true,
            });
        }
        assert_eq!(stdout.text, "• before_prompt hook blocked\n\n  (empty)\n\n");
    }

    #[test]
    fn writes_assistant_tool_and_result_json_lines_in_protocol_order() {
        let mut stdout = Capture::default();
        {
            let mut writer = PromptJsonWriter::new(&mut stdout);
            writer.write_assistant_delta("checking");
            writer.write_tool_call(
                "tc_1",
                "Shell",
                &PromptValue::Json(serde_json::json!({ "command": "ls" })),
            );
            writer.write_tool_result(
                "tc_1",
                &PromptValue::String("file1.py\nfile2.py".to_owned()),
            );
            writer.write_assistant_delta("done");
            writer.finish();
        }
        assert_eq!(
            stdout.text,
            concat!(
                "{\"role\":\"assistant\",\"content\":\"checking\",\"tool_calls\":[{\"type\":\"function\",\"id\":\"tc_1\",\"function\":{\"name\":\"Shell\",\"arguments\":\"{\\\"command\\\":\\\"ls\\\"}\"}}]}\n",
                "{\"role\":\"tool\",\"tool_call_id\":\"tc_1\",\"content\":\"file1.py\\nfile2.py\"}\n",
                "{\"role\":\"assistant\",\"content\":\"done\"}\n",
            )
        );
    }

    #[test]
    fn merges_tool_call_deltas_and_can_discard_failed_output() {
        let mut stdout = Capture::default();
        {
            let mut writer = PromptJsonWriter::new(&mut stdout);
            writer.write_assistant_delta("failed attempt");
            writer.discard_assistant();
            writer.write_tool_call_delta("tc_2", Some("ReadFile"), Some("{\"path\":"));
            writer.write_tool_call_delta("tc_2", None, Some("\"a.txt\"}"));
            writer.finish();
        }
        assert_eq!(
            stdout.text,
            "{\"role\":\"assistant\",\"tool_calls\":[{\"type\":\"function\",\"id\":\"tc_2\",\"function\":{\"name\":\"ReadFile\",\"arguments\":\"{\\\"path\\\":\\\"a.txt\\\"}\"}}]}\n"
        );
    }

    #[test]
    fn emits_retry_version_and_resume_meta_messages() {
        let mut stdout = Capture::default();
        let mut stderr = Capture::default();
        {
            let mut writer = PromptJsonWriter::new(&mut stdout);
            writer.write_retrying(&RetryingEvent {
                failed_attempt: 1,
                next_attempt: 2,
                max_attempts: 3,
                delay_ms: 300,
                error_name: "RateLimit".to_owned(),
                error_message: "status=429".to_owned(),
                status_code: Some(429),
            });
        }
        write_experimental_version(
            "1.2.3",
            PromptOutputFormat::StreamJson,
            &mut stdout,
            &mut stderr,
        );
        write_resume_hint(
            "ses_1",
            PromptOutputFormat::StreamJson,
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(
            stdout.text,
            concat!(
                "{\"role\":\"meta\",\"type\":\"turn.step.retrying\",\"failed_attempt\":1,\"next_attempt\":2,\"max_attempts\":3,\"delay_ms\":300,\"error_name\":\"RateLimit\",\"error_message\":\"status=429\",\"status_code\":429}\n",
                "{\"role\":\"meta\",\"type\":\"system.version\",\"version\":\"1.2.3\"}\n",
                "{\"role\":\"meta\",\"type\":\"session.resume_hint\",\"session_id\":\"ses_1\",\"command\":\"kimi -r ses_1\",\"content\":\"To resume this session: kimi -r ses_1\"}\n",
            )
        );
        assert!(stderr.text.is_empty());
    }
}

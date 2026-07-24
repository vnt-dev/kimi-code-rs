//! Edit business rules without filesystem I/O.
//!
//! Original: `packages/agent-core-v2/src/app/edit/editService.ts`.

use super::TextModel;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EditApplyInput {
    pub path: String,
    pub old_string: String,
    pub new_string: String,
    pub replace_all: bool,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EditApplyResult {
    Ok { raw_content: String, count: usize },
    Err { error: String },
}
#[derive(Default)]
pub struct EditService;
impl EditService {
    pub fn apply(&self, model: &TextModel, input: &EditApplyInput) -> EditApplyResult {
        if input.replace_all {
            let (text, count) = model.replace_all(&input.old_string, &input.new_string);
            if count == 0 {
                return EditApplyResult::Err {
                    error: not_found(&input.path),
                };
            }
            return EditApplyResult::Ok {
                raw_content: model.materialize(&text),
                count,
            };
        }
        let count = model.count_occurrences(&input.old_string);
        if count == 0 {
            return EditApplyResult::Err {
                error: not_found(&input.path),
            };
        }
        if count > 1 {
            return EditApplyResult::Err {
                error: not_unique(&input.path, count),
            };
        }
        EditApplyResult::Ok {
            raw_content: model
                .materialize(&model.replace_once(&input.old_string, &input.new_string)),
            count: 1,
        }
    }
}
fn not_found(path: &str) -> String {
    format!(
        "old_string not found in {path}, the file contents may be out of date. Please use the Read Tool to reload the content.\n"
    )
}
fn not_unique(path: &str, count: usize) -> String {
    format!(
        "old_string is not unique in {path} (found {count} occurrences). To replace every occurrence, set replace_all=true. To replace only one occurrence, include more surrounding context in old_string."
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_missing_and_ambiguous_before_writing() {
        let service = EditService;
        let model = TextModel::new("x\nx\n");
        let input = EditApplyInput {
            path: "a.txt".into(),
            old_string: "x".into(),
            new_string: "y".into(),
            replace_all: false,
        };
        assert!(
            matches!(service.apply(&model,&input),EditApplyResult::Err{error}if error.contains("not unique"))
        );
        let missing = EditApplyInput {
            old_string: "z".into(),
            ..input
        };
        assert!(
            matches!(service.apply(&model,&missing),EditApplyResult::Err{error}if error.ends_with("content.\n"))
        );
    }
    #[test]
    fn replace_all_preserves_crlf() {
        let service = EditService;
        let model = TextModel::new("a\r\na\r\n");
        let input = EditApplyInput {
            path: "a".into(),
            old_string: "a\n".into(),
            new_string: "b\n".into(),
            replace_all: true,
        };
        assert_eq!(
            service.apply(&model, &input),
            EditApplyResult::Ok {
                raw_content: "b\r\nb\r\n".into(),
                count: 2
            }
        );
    }
}

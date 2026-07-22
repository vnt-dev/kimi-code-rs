// Original:
//   packages/agent-core-v2/src/kosong/provider/bases/merge-user-messages.ts
//   mergeConsecutiveUserMessages() policy object.
pub trait ConsecutiveUserMessageMergePolicy<T> {
    fn is_user(&self, message: &T) -> bool;
    fn is_tool_result_only(&self, message: &T) -> bool;
    fn merge(&self, last: T, next: T) -> T;
}

// Original: merge-user-messages.ts, mergeConsecutiveUserMessages()
pub fn merge_consecutive_user_messages<T: Clone>(
    messages: &[T],
    policy: &impl ConsecutiveUserMessageMergePolicy<T>,
) -> Vec<T> {
    let mut output: Vec<T> = Vec::new();
    for message in messages.iter().cloned() {
        let should_merge = output.last().is_some_and(|last| {
            policy.is_user(last)
                && policy.is_user(&message)
                && (policy.is_tool_result_only(last) || !policy.is_tool_result_only(&message))
        });
        if should_merge {
            let last = output.pop().expect("last exists when should_merge is true");
            output.push(policy.merge(last, message));
        } else {
            output.push(message);
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Entry {
        role: &'static str,
        tool_only: bool,
        content: String,
    }

    struct Policy;

    impl ConsecutiveUserMessageMergePolicy<Entry> for Policy {
        fn is_user(&self, message: &Entry) -> bool {
            message.role == "user"
        }

        fn is_tool_result_only(&self, message: &Entry) -> bool {
            message.tool_only
        }

        fn merge(&self, mut last: Entry, next: Entry) -> Entry {
            last.content.push('|');
            last.content.push_str(&next.content);
            last.tool_only = last.tool_only && next.tool_only;
            last
        }
    }

    fn user(content: &str, tool_only: bool) -> Entry {
        Entry {
            role: "user",
            tool_only,
            content: content.to_owned(),
        }
    }

    fn assistant(content: &str) -> Entry {
        Entry {
            role: "assistant",
            tool_only: false,
            content: content.to_owned(),
        }
    }

    #[test]
    fn merges_plain_consecutive_user_messages_and_chains_the_fold() {
        let output = merge_consecutive_user_messages(
            &[user("a", false), user("b", false), user("c", false)],
            &Policy,
        );
        assert_eq!(output, [user("a|b|c", false)]);
    }

    #[test]
    fn never_merges_plain_user_into_a_following_tool_result_only_message() {
        let output =
            merge_consecutive_user_messages(&[user("plain", false), user("tool", true)], &Policy);
        assert_eq!(output, [user("plain", false), user("tool", true)]);
    }

    #[test]
    fn merges_after_tool_result_only_and_between_tool_results() {
        let tool_then_plain =
            merge_consecutive_user_messages(&[user("tool", true), user("plain", false)], &Policy);
        assert_eq!(tool_then_plain, [user("tool|plain", false)]);

        let tool_chain =
            merge_consecutive_user_messages(&[user("one", true), user("two", true)], &Policy);
        assert_eq!(tool_chain, [user("one|two", true)]);
    }

    #[test]
    fn role_boundaries_and_empty_input_preserve_order() {
        let input = [user("a", false), assistant("x"), user("b", false)];
        assert_eq!(merge_consecutive_user_messages(&input, &Policy), input);
        assert!(merge_consecutive_user_messages::<Entry>(&[], &Policy).is_empty());
    }
}

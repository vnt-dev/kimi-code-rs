export interface PromptDraft<Attachment, Skill> {
  text: string;
  attachments: Attachment[];
  skills: Skill[];
}

export type PromptDrafts<Attachment, Skill> = Record<
  string,
  PromptDraft<Attachment, Skill>
>;

export type PromptDraftUpdater<Value> = Value | ((current: Value) => Value);

export function promptDraftFor<Attachment, Skill>(
  drafts: PromptDrafts<Attachment, Skill>,
  conversationId: string | undefined,
): PromptDraft<Attachment, Skill> {
  if (conversationId && drafts[conversationId]) return drafts[conversationId];
  return { text: "", attachments: [], skills: [] };
}

export function updatePromptDraft<
  Attachment,
  Skill,
  Key extends keyof PromptDraft<Attachment, Skill>,
>(
  drafts: PromptDrafts<Attachment, Skill>,
  conversationId: string,
  key: Key,
  update: PromptDraftUpdater<PromptDraft<Attachment, Skill>[Key]>,
): PromptDrafts<Attachment, Skill> {
  const draft = promptDraftFor(drafts, conversationId);
  const currentValue = draft[key];
  const nextValue =
    typeof update === "function"
      ? (
          update as (
            current: PromptDraft<Attachment, Skill>[Key],
          ) => PromptDraft<Attachment, Skill>[Key]
        )(currentValue)
      : update;
  if (Object.is(currentValue, nextValue)) return drafts;
  return {
    ...drafts,
    [conversationId]: {
      ...draft,
      [key]: nextValue,
    },
  };
}

export function removePromptDrafts<Attachment, Skill>(
  drafts: PromptDrafts<Attachment, Skill>,
  conversationIds: ReadonlySet<string>,
): PromptDrafts<Attachment, Skill> {
  let changed = false;
  const next = { ...drafts };
  for (const conversationId of conversationIds) {
    if (!(conversationId in next)) continue;
    delete next[conversationId];
    changed = true;
  }
  return changed ? next : drafts;
}

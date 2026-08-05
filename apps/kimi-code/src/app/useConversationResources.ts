import {
  type ChangeEvent,
  type ClipboardEvent,
  type Dispatch,
  type RefObject,
  type SetStateAction,
  useEffect,
} from "react";
import {
  createAgentClient,
  getSkillContent,
  listSkills,
} from "../agentRpc";
import {
  completedTurnMessageId,
  type RenderMessage,
} from "../chat/history";
import type {
  InFlightTurn,
  PromptAttachment,
} from "../chat/liveTurns";
import {
  type CompactionSummaryDetail,
  type SkillDetailTarget,
} from "../components/sidebars/ChatSidebars";
import { t } from "../i18n";
import { projectLiveUserMessage } from "../liveUserMessage";
import { messageText } from "../chat/messages";
import {
  MAX_PROMPT_ATTACHMENTS,
  preparePromptAttachment,
  promptAttachmentKind,
} from "../prompt/attachments";
import { parseSkillPromptDisplay } from "../prompt/skills";
import type { PromptDraftUpdater } from "../promptDrafts";
import { invoke } from "../transport";
import type {
  AccountUsage,
  AuthStatus,
  CompactionEvent,
  Conversation,
  DeviceCode,
  Model,
  SkillContent,
  SkillDescriptor,
  TurnFileChange,
} from "../types";
import { conciseError } from "../utils/errors";
import {
  MAX_PROMPT_SKILLS,
  fetchConversationHistory,
  type ConversationHistory,
} from "./appUtils";

type Setter<T> = Dispatch<SetStateAction<T>>;

interface ConversationResourceOptions {
  accountUsageRequest: RefObject<number>;
  activeAgentScope?: { sessionId: string; agentId: string };
  activeCompaction?: CompactionEvent;
  activeConversation?: Conversation;
  activeConversationIdRef: RefObject<string | undefined>;
  availableSkills: SkillDescriptor[];
  closeSideChat: () => void;
  composerAddOpen: boolean;
  historyRequests: RefObject<Record<string, number>>;
  inFlightTurnsRef: RefObject<Record<string, InFlightTurn>>;
  promptAttachments: PromptAttachment[];
  promptSkills: SkillDescriptor[];
  refreshModels: () => Promise<void>;
  resetPrompt: (value?: string, conversationId?: string) => void;
  selectedModel?: Model;
  setAccountUsage: Setter<AccountUsage | undefined>;
  setAccountUsageBusy: Setter<boolean>;
  setAccountUsageError: Setter<string | undefined>;
  setAuth: Setter<AuthStatus>;
  setAvailableSkills: Setter<SkillDescriptor[]>;
  setCompactionHistoryReady: Setter<Record<string, boolean>>;
  setCompactionSummaryDetail: Setter<CompactionSummaryDetail | undefined>;
  setComposerAddOpen: Setter<boolean>;
  setDeviceCode: Setter<DeviceCode | undefined>;
  setHistoryByConversation: Setter<Record<string, ConversationHistory>>;
  setLoginBusy: Setter<boolean>;
  setLoginOpen: Setter<boolean>;
  setMessageDurations: Setter<Record<string, Record<string, number>>>;
  setMessageFileChanges: Setter<
    Record<string, Record<string, readonly TurnFileChange[]>>
  >;
  setProfileOpen: Setter<boolean>;
  setPromptAttachments: (
    update: PromptDraftUpdater<PromptAttachment[]>,
    conversationId?: string,
  ) => void;
  setPromptSkills: (
    update: PromptDraftUpdater<SkillDescriptor[]>,
    conversationId?: string,
  ) => void;
  setSkillDetail: Setter<SkillContent | undefined>;
  setSkillDetailBusy: Setter<boolean>;
  setSkillDetailError: Setter<string | undefined>;
  setSkillDetailTarget: Setter<SkillDetailTarget | undefined>;
  setSkillsBusy: Setter<boolean>;
  setSkillsError: Setter<string | undefined>;
  setUndoMessageBusy: Setter<boolean>;
  setUndoMessageTarget: Setter<RenderMessage | undefined>;
  showNotice: (message: string) => void;
  skillDetailRequest: RefObject<number>;
  skillsRequest: RefObject<number>;
  textareaRef: RefObject<HTMLTextAreaElement | null>;
  undoableUserMessageId?: string;
  undoMessageBusy: boolean;
  undoMessageTarget?: RenderMessage;
}

export function useConversationResources({
  accountUsageRequest,
  activeAgentScope,
  activeCompaction,
  activeConversation,
  activeConversationIdRef,
  availableSkills,
  closeSideChat,
  composerAddOpen,
  historyRequests,
  inFlightTurnsRef,
  promptAttachments,
  promptSkills,
  refreshModels,
  resetPrompt,
  selectedModel,
  setAccountUsage,
  setAccountUsageBusy,
  setAccountUsageError,
  setAuth,
  setAvailableSkills,
  setCompactionHistoryReady,
  setCompactionSummaryDetail,
  setComposerAddOpen,
  setDeviceCode,
  setHistoryByConversation,
  setLoginBusy,
  setLoginOpen,
  setMessageDurations,
  setMessageFileChanges,
  setProfileOpen,
  setPromptAttachments,
  setPromptSkills,
  setSkillDetail,
  setSkillDetailBusy,
  setSkillDetailError,
  setSkillDetailTarget,
  setSkillsBusy,
  setSkillsError,
  setUndoMessageBusy,
  setUndoMessageTarget,
  showNotice,
  skillDetailRequest,
  skillsRequest,
  textareaRef,
  undoableUserMessageId,
  undoMessageBusy,
  undoMessageTarget,
}: ConversationResourceOptions) {
  const startLogin = async (): Promise<void> => {
    setProfileOpen(false);
    setLoginOpen(true);
    setLoginBusy(true);
    setDeviceCode(undefined);
    try {
      const status = await invoke<AuthStatus>("login");
      setAuth(status);
      if (status.loggedIn) {
        setLoginOpen(false);
        showNotice(t("notice.loginSuccess"));
        void refreshModels();
      }
    } catch (error) {
      showNotice(conciseError(error));
    } finally {
      setLoginBusy(false);
    }
  };

  const signOut = async (): Promise<void> => {
    try {
      const status = await invoke<AuthStatus>("logout");
      setAuth(status);
      accountUsageRequest.current += 1;
      setAccountUsage(undefined);
      setAccountUsageBusy(false);
      setAccountUsageError(undefined);
      setProfileOpen(false);
      showNotice(t("notice.logoutSuccess"));
    } catch (error) {
      showNotice(conciseError(error));
    }
  };

  const refreshHistory = async (
    conversationId: string,
    completedTurn?: InFlightTurn,
  ): Promise<boolean> => {
    const request = (historyRequests.current[conversationId] ?? 0) + 1;
    historyRequests.current[conversationId] = request;
    try {
      const page = await fetchConversationHistory(conversationId);
      if (request !== historyRequests.current[conversationId]) return false;
      const items = [...page.items].reverse();
      const durationMs = completedTurn?.durationMs;
      const fileChanges = completedTurn?.fileChanges;
      if (
        completedTurn &&
        (durationMs !== undefined || (fileChanges?.length ?? 0) > 0)
      ) {
        const messageId = completedTurnMessageId(items, completedTurn);
        if (messageId) {
          if (durationMs !== undefined) {
            setMessageDurations((current) => ({
              ...current,
              [conversationId]: {
                ...current[conversationId],
                [messageId]: durationMs,
              },
            }));
          }
          if (fileChanges && fileChanges.length > 0) {
            setMessageFileChanges((current) => ({
              ...current,
              [conversationId]: {
                ...current[conversationId],
                [messageId]: fileChanges,
              },
            }));
          }
        }
      }
      setHistoryByConversation((current) => ({
        ...current,
        [conversationId]: {
          conversationId,
          items,
          loading: false,
        },
      }));
      return true;
    } catch (error) {
      if (request !== historyRequests.current[conversationId]) return false;
      const message = conciseError(error);
      setHistoryByConversation((current) => ({
        ...current,
        [conversationId]: {
          conversationId,
          items: current[conversationId]?.items ?? [],
          loading: false,
          error: message,
        },
      }));
      showNotice(message);
      return false;
    }
  };

  const confirmUndoMessage = async (): Promise<void> => {
    const target = undoMessageTarget;
    const conversation = activeConversation;
    const scope = activeAgentScope;
    if (!target || !conversation || !scope || undoMessageBusy) return;
    if (
      scope.sessionId !== conversation.id ||
      target.id !== undoableUserMessageId ||
      inFlightTurnsRef.current[conversation.id] !== undefined
    ) {
      setUndoMessageTarget(undefined);
      showNotice(t("undo.unavailable"));
      return;
    }

    setUndoMessageBusy(true);
    try {
      await createAgentClient(scope).undoHistory(1);
      const projected = projectLiveUserMessage({
        promptId: target.prompt_id ?? target.id,
        userMessageId: target.id,
        createdAt: target.created_at,
        content: target.content,
        origin: target.metadata?.origin,
      });
      const display = parseSkillPromptDisplay(projected.text);
      await refreshHistory(conversation.id);
      resetPrompt(display.text, conversation.id);
      setPromptAttachments(projected.attachments, conversation.id);
      setPromptSkills(
        availableSkills.filter((skill) => display.skills.includes(skill.name)),
        conversation.id,
      );
      setUndoMessageTarget(undefined);
      showNotice(t("undo.success"));
      window.requestAnimationFrame(() => {
        if (activeConversationIdRef.current !== conversation.id) return;
        const textarea = textareaRef.current;
        if (!textarea) return;
        textarea.focus();
        textarea.setSelectionRange(display.text.length, display.text.length);
      });
    } catch (error) {
      showNotice(conciseError(error));
    } finally {
      setUndoMessageBusy(false);
    }
  };

  useEffect(() => {
    const conversationId = activeConversation?.id;
    if (!conversationId || activeCompaction?.phase !== "completed") return;
    void refreshHistory(conversationId).then((refreshed) => {
      if (!refreshed) return;
      setCompactionHistoryReady((current) => ({
        ...current,
        [conversationId]: true,
      }));
    });
  }, [activeConversation?.id, activeCompaction?.phase]);

  const loadAvailableSkills = async (): Promise<void> => {
    const request = skillsRequest.current + 1;
    skillsRequest.current = request;
    const scope = activeAgentScope;
    if (!scope) {
      setAvailableSkills([]);
      setSkillsBusy(false);
      setSkillsError(t("notice.sessionPreparing"));
      return;
    }

    setSkillsBusy(true);
    setSkillsError(undefined);
    try {
      const skills = await listSkills(scope.sessionId);
      if (request !== skillsRequest.current) return;
      setAvailableSkills(skills);
    } catch (error) {
      if (request !== skillsRequest.current) return;
      setAvailableSkills([]);
      setSkillsError(conciseError(error));
    } finally {
      if (request === skillsRequest.current) setSkillsBusy(false);
    }
  };

  const toggleComposerAdd = (): void => {
    if (composerAddOpen) {
      setComposerAddOpen(false);
      return;
    }
    setComposerAddOpen(true);
    void loadAvailableSkills();
  };

  const selectPromptSkill = (skill: SkillDescriptor): void => {
    const selected = promptSkills.some(
      (item) => item.name === skill.name,
    );
    if (!selected && promptSkills.length >= MAX_PROMPT_SKILLS) {
      showNotice(t("notice.maxSkills", { count: MAX_PROMPT_SKILLS }));
      setComposerAddOpen(false);
      return;
    }
    setPromptSkills((current) =>
      selected
        ? current.filter((item) => item.name !== skill.name)
        : [...current, skill],
    );
    setComposerAddOpen(false);
    window.requestAnimationFrame(() => textareaRef.current?.focus());
  };

  const openSkillDetail = async (skill: SkillDetailTarget): Promise<void> => {
    const request = skillDetailRequest.current + 1;
    skillDetailRequest.current = request;
    const scope = activeAgentScope;

    setComposerAddOpen(false);
    closeSideChat();
    setCompactionSummaryDetail(undefined);
    setSkillDetailTarget(skill);
    setSkillDetail(undefined);
    setSkillDetailError(undefined);
    if (!scope) {
      setSkillDetailBusy(false);
      setSkillDetailError(t("notice.sessionPreparing"));
      return;
    }

    setSkillDetailBusy(true);
    try {
      const content = await getSkillContent(scope.sessionId, skill.name);
      if (request !== skillDetailRequest.current) return;
      setSkillDetail(content);
    } catch (error) {
      if (request !== skillDetailRequest.current) return;
      setSkillDetailError(conciseError(error));
    } finally {
      if (request === skillDetailRequest.current) setSkillDetailBusy(false);
    }
  };

  const closeSkillDetail = (): void => {
    skillDetailRequest.current += 1;
    setSkillDetailTarget(undefined);
    setSkillDetail(undefined);
    setSkillDetailBusy(false);
    setSkillDetailError(undefined);
  };

  const openCompactionSummary = (message: RenderMessage): void => {
    closeSideChat();
    skillDetailRequest.current += 1;
    setSkillDetailTarget(undefined);
    setSkillDetail(undefined);
    setSkillDetailBusy(false);
    setSkillDetailError(undefined);
    setCompactionSummaryDetail({
      id: message.id,
      content: messageText(message),
      createdAt: message.created_at,
    });
  };

  const addPromptAttachments = async (
    files: readonly File[],
  ): Promise<void> => {
    if (files.length === 0) return;
    const conversationId = activeConversation?.id;
    if (!conversationId) return;
    const remaining = MAX_PROMPT_ATTACHMENTS - promptAttachments.length;
    if (remaining <= 0) {
      showNotice(t("notice.maxAttachments", { count: MAX_PROMPT_ATTACHMENTS }));
      return;
    }

    const selected = files.slice(0, remaining);
    const prepared: PromptAttachment[] = [];
    for (const file of selected) {
      try {
        const kind = promptAttachmentKind(file.type);
        if (kind === "image" && !selectedModel?.supportsImage) {
          throw new Error(t("error.imageNotSupported"));
        }
        if (kind === "video" && !selectedModel?.supportsVideo) {
          throw new Error(t("error.videoNotSupported"));
        }
        prepared.push(await preparePromptAttachment(file));
      } catch (error) {
        showNotice(conciseError(error));
      }
    }
    if (prepared.length > 0) {
      setPromptAttachments(
        (current) => [...current, ...prepared],
        conversationId,
      );
    }
    if (files.length > remaining) {
      showNotice(t("notice.maxAttachments", { count: MAX_PROMPT_ATTACHMENTS }));
    }
  };

  const handleAttachmentInput = (
    event: ChangeEvent<HTMLInputElement>,
  ): void => {
    const files = Array.from(event.target.files ?? []);
    event.target.value = "";
    void addPromptAttachments(files);
  };

  const handlePromptPaste = (
    event: ClipboardEvent<HTMLTextAreaElement>,
  ): void => {
    const media = Array.from(event.clipboardData.items)
      .filter((item) => item.kind === "file")
      .map((item) => item.getAsFile())
      .filter((file): file is File => file !== null);
    if (media.length > 0) void addPromptAttachments(media);
  };

  return {
    startLogin,
    signOut,
    refreshHistory,
    confirmUndoMessage,
    loadAvailableSkills,
    toggleComposerAdd,
    selectPromptSkill,
    openSkillDetail,
    closeSkillDetail,
    openCompactionSummary,
    handleAttachmentInput,
    handlePromptPaste,
  };
}

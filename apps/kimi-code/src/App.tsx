import {
  type FormEvent,
  type KeyboardEvent,
  type MouseEvent,
  type ReactNode,
  isValidElement,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  ArrowUp,
  Bot,
  BrainCircuit,
  Check,
  ChevronDown,
  ChevronRight,
  CircleUserRound,
  Code2,
  Copy,
  ExternalLink,
  FileCode2,
  Folder,
  FolderGit2,
  LogIn,
  LogOut,
  Menu,
  MessageSquareText,
  Minimize2,
  MoreHorizontal,
  PanelLeftClose,
  PanelLeftOpen,
  Plus,
  RefreshCw,
  ShieldAlert,
  ShieldCheck,
  Sparkles,
  SquarePen,
  TerminalSquare,
  Wrench,
  X,
} from "lucide-react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";

import {
  createId,
  getActive,
  loadState,
  newConversation,
  newProject,
  persistState,
} from "./store";
import type {
  AgentCompactionEvent,
  AgentInteraction,
  AgentInteractionsEvent,
  ApprovalPayload,
  AuthStatus,
  ChatMessage,
  ChatStreamEvent,
  CompactionEvent,
  DesktopState,
  DeviceCode,
  Model,
  PermissionMode,
  Project,
} from "./types";

const PROMPT_SUGGESTIONS = [
  {
    icon: <FileCode2 size={17} />,
    title: "理解这个项目",
    prompt: "分析这个项目的结构、核心模块和运行方式，给我一份简洁的导览。",
  },
  {
    icon: <Wrench size={17} />,
    title: "排查一个问题",
    prompt: "帮我检查这个项目中潜在的错误和可维护性问题，按优先级给出建议。",
  },
  {
    icon: <TerminalSquare size={17} />,
    title: "开始一个功能",
    prompt: "先阅读项目结构，然后帮我规划并实现一个新功能。",
  },
];

function formatTime(timestamp: number): string {
  return new Intl.DateTimeFormat("zh-CN", {
    hour: "2-digit",
    minute: "2-digit",
  }).format(timestamp);
}

function formatContext(value: number): string {
  if (value >= 1_000_000) return `${Math.round(value / 1_000_000)}M`;
  if (value >= 1_000) return `${Math.round(value / 1_000)}K`;
  return `${value}`;
}

function conciseError(error: unknown): string {
  const message = error instanceof Error ? error.message : String(error);
  return message.replace(/^Error:\s*/i, "");
}

export default function App() {
  const [desktop, setDesktop] = useState<DesktopState>(() => loadState());
  const [auth, setAuth] = useState<AuthStatus>({
    loggedIn: false,
    provider: "kimi-code",
  });
  const [models, setModels] = useState<Model[]>([]);
  const [prompt, setPrompt] = useState("");
  const [effort, setEffort] = useState("medium");
  const [sidebarCollapsed, setSidebarCollapsed] = useState(false);
  const [loginOpen, setLoginOpen] = useState(false);
  const [loginBusy, setLoginBusy] = useState(false);
  const [deviceCode, setDeviceCode] = useState<DeviceCode>();
  const [profileOpen, setProfileOpen] = useState(false);
  const [modelsBusy, setModelsBusy] = useState(false);
  const [notice, setNotice] = useState<string>();
  const [copiedMessage, setCopiedMessage] = useState<string>();
  const [interactions, setInteractions] = useState<
    Record<string, AgentInteraction[]>
  >({});
  const [resolvingInteraction, setResolvingInteraction] = useState<string>();
  const [compactions, setCompactions] = useState<
    Record<string, CompactionEvent>
  >({});
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const noticeTimer = useRef<number | undefined>(undefined);

  const { project: activeProject, conversation: activeConversation } = useMemo(
    () => getActive(desktop),
    [desktop],
  );
  const selectedModel =
    models.find((model) => model.id === activeConversation?.modelId) ?? models[0];
  const permissionMode = activeConversation?.permissionMode ?? "manual";
  const isStreaming = activeConversation?.messages.some(
    (message) => message.status === "streaming",
  );
  const activeApproval = activeConversation
    ? interactions[activeConversation.id]?.find(
        (interaction) => interaction.kind === "approval",
      )
    : undefined;
  const activeCompaction = activeConversation
    ? compactions[activeConversation.id]
    : undefined;

  const updateDesktop = (
    recipe: (current: DesktopState) => DesktopState,
  ): void => {
    setDesktop((current) => recipe(current));
  };

  const showNotice = (message: string): void => {
    setNotice(message);
    if (noticeTimer.current) window.clearTimeout(noticeTimer.current);
    noticeTimer.current = window.setTimeout(() => setNotice(undefined), 3600);
  };

  const loadModels = async (): Promise<void> => {
    setModelsBusy(true);
    try {
      const nextModels = await invoke<Model[]>("list_models");
      setModels(nextModels);
      if (nextModels.length === 0) showNotice("当前账号没有可用模型");
    } catch (error) {
      showNotice(conciseError(error));
    } finally {
      setModelsBusy(false);
    }
  };

  useEffect(() => {
    persistState(desktop);
  }, [desktop]);

  useEffect(() => {
    let active = true;
    invoke<AuthStatus>("auth_status")
      .then((status) => {
        if (!active) return;
        setAuth(status);
        if (status.loggedIn) void loadModels();
      })
      .catch(() => {
        // Vite's browser preview has no Tauri bridge; the actual desktop app does.
      });
    return () => {
      active = false;
    };
  }, []);

  useEffect(() => {
    const unlistenDevice = listen<DeviceCode>("auth-device-code", (event) => {
      setDeviceCode(event.payload);
      setLoginOpen(true);
    });
    const unlistenBrowserError = listen<string>(
      "auth-browser-open-failed",
      (event) => {
        showNotice(`未能自动打开浏览器：${event.payload}`);
      },
    );
    const unlistenStream = listen<ChatStreamEvent>("chat-stream", (event) => {
      const delta = event.payload;
      updateDesktop((current) => ({
        ...current,
        projects: current.projects.map((project) => ({
          ...project,
          conversations: project.conversations.map((conversation) => {
            if (conversation.id !== delta.conversationId) return conversation;
            const messages = [...conversation.messages];
            let index = messages.length - 1;
            while (
              index >= 0 &&
              !(
                messages[index].role === "assistant" &&
                messages[index].status === "streaming"
              )
            ) {
              index -= 1;
            }
            if (index < 0) return conversation;
            const message = messages[index];
            messages[index] = {
              ...message,
              content:
                delta.kind === "text"
                  ? message.content + delta.content
                  : message.content,
              thinking:
                delta.kind === "thinking"
                  ? (message.thinking ?? "") + delta.content
                  : message.thinking,
            };
            return { ...conversation, messages, updatedAt: Date.now() };
          }),
        })),
      }));
    });
    const unlistenInteractions = listen<AgentInteractionsEvent>(
      "agent-interactions",
      (event) => {
        setInteractions((current) => ({
          ...current,
          [event.payload.conversationId]: event.payload.interactions,
        }));
      },
    );
    const unlistenCompaction = listen<AgentCompactionEvent>(
      "agent-compaction",
      (event) => {
        setCompactions((current) => ({
          ...current,
          [event.payload.conversationId]: event.payload.event,
        }));
      },
    );
    return () => {
      void unlistenDevice.then((unlisten) => unlisten());
      void unlistenBrowserError.then((unlisten) => unlisten());
      void unlistenStream.then((unlisten) => unlisten());
      void unlistenInteractions.then((unlisten) => unlisten());
      void unlistenCompaction.then((unlisten) => unlisten());
    };
  }, []);

  useEffect(() => {
    const scroll = scrollRef.current;
    if (scroll) scroll.scrollTo({ top: scroll.scrollHeight, behavior: "smooth" });
  }, [activeConversation?.messages, activeCompaction?.phase]);

  useEffect(() => {
    const textarea = textareaRef.current;
    if (!textarea) return;
    textarea.style.height = "0px";
    textarea.style.height = `${Math.min(textarea.scrollHeight, 180)}px`;
  }, [prompt]);

  const addProject = async (): Promise<void> => {
    try {
      const selection = await open({
        directory: true,
        multiple: false,
        title: "选择一个项目目录",
      });
      if (!selection) return;
      const existing = desktop.projects.find((item) => item.path === selection);
      if (existing) {
        updateDesktop((current) => ({
          ...current,
          activeProjectId: existing.id,
          activeConversationId: existing.conversations[0]?.id,
          projects: current.projects.map((item) => ({
            ...item,
            expanded: item.id === existing.id ? true : item.expanded,
          })),
        }));
        return;
      }
      const project = newProject(selection, desktop.projects.length);
      updateDesktop((current) => ({
        projects: [...current.projects, project],
        activeProjectId: project.id,
        activeConversationId: project.conversations[0].id,
      }));
      setSidebarCollapsed(false);
    } catch (error) {
      showNotice(conciseError(error));
    }
  };

  const createConversation = (
    project: Project,
    event?: MouseEvent<HTMLButtonElement>,
  ): void => {
    event?.stopPropagation();
    const conversation = newConversation();
    updateDesktop((current) => ({
      ...current,
      activeProjectId: project.id,
      activeConversationId: conversation.id,
      projects: current.projects.map((item) =>
        item.id === project.id
          ? {
              ...item,
              expanded: true,
              conversations: [conversation, ...item.conversations],
            }
          : item,
      ),
    }));
    setPrompt("");
  };

  const selectConversation = (
    projectId: string,
    conversationId: string,
  ): void => {
    updateDesktop((current) => ({
      ...current,
      activeProjectId: projectId,
      activeConversationId: conversationId,
    }));
  };

  const toggleProject = (projectId: string): void => {
    updateDesktop((current) => ({
      ...current,
      projects: current.projects.map((project) =>
        project.id === projectId
          ? { ...project, expanded: !project.expanded }
          : project,
      ),
    }));
  };

  const chooseModel = (modelId: string): void => {
    if (!activeConversation || !activeProject) return;
    updateDesktop((current) => ({
      ...current,
      projects: current.projects.map((project) =>
        project.id === activeProject.id
          ? {
              ...project,
              conversations: project.conversations.map((conversation) =>
                conversation.id === activeConversation.id
                  ? { ...conversation, modelId }
                  : conversation,
              ),
            }
          : project,
      ),
    }));
    const model = models.find((item) => item.id === modelId);
    if (model?.defaultEffort) setEffort(model.defaultEffort);
  };

  const choosePermissionMode = (mode: PermissionMode): void => {
    if (!activeConversation || !activeProject) return;
    updateDesktop((current) => ({
      ...current,
      projects: current.projects.map((project) =>
        project.id !== activeProject.id
          ? project
          : {
              ...project,
              conversations: project.conversations.map((conversation) =>
                conversation.id === activeConversation.id
                  ? { ...conversation, permissionMode: mode }
                  : conversation,
              ),
            },
      ),
    }));
  };

  const startLogin = async (): Promise<void> => {
    setLoginOpen(true);
    setLoginBusy(true);
    setDeviceCode(undefined);
    try {
      const status = await invoke<AuthStatus>("login");
      setAuth(status);
      if (status.loggedIn) {
        setLoginOpen(false);
        showNotice("已登录 Kimi Code");
        await loadModels();
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
      setModels([]);
      setProfileOpen(false);
      showNotice("已退出登录");
    } catch (error) {
      showNotice(conciseError(error));
    }
  };

  const setConversationResult = (
    projectId: string,
    conversationId: string,
    assistantId: string,
    status: "done" | "error",
    fallback?: string,
  ): void => {
    updateDesktop((current) => ({
      ...current,
      projects: current.projects.map((project) =>
        project.id !== projectId
          ? project
          : {
              ...project,
              conversations: project.conversations.map((conversation) =>
                conversation.id !== conversationId
                  ? conversation
                  : {
                      ...conversation,
                      updatedAt: Date.now(),
                      messages: conversation.messages.map((message) =>
                        message.id !== assistantId
                          ? message
                          : {
                              ...message,
                              content: message.content || fallback || "",
                              status,
                            },
                      ),
                    },
              ),
            },
      ),
    }));
  };

  const sendPrompt = async (override?: string): Promise<void> => {
    const text = (override ?? prompt).trim();
    if (!text || !activeProject || !activeConversation || isStreaming) return;
    if (!auth.loggedIn) {
      void startLogin();
      return;
    }
    if (!selectedModel) {
      showNotice("请先同步并选择一个模型");
      return;
    }

    const userMessage: ChatMessage = {
      id: createId("message"),
      role: "user",
      content: text,
      createdAt: Date.now(),
      status: "done",
    };
    const assistantMessage: ChatMessage = {
      id: createId("message"),
      role: "assistant",
      content: "",
      thinking: "",
      createdAt: Date.now(),
      status: "streaming",
    };
    const history = [...activeConversation.messages, userMessage].map(
      ({ role, content }) => ({ role, content }),
    );
    const title =
      activeConversation.messages.length === 0
        ? text.replace(/\s+/g, " ").slice(0, 28)
        : activeConversation.title;

    setCompactions((current) => {
      if (!(activeConversation.id in current)) return current;
      const next = { ...current };
      delete next[activeConversation.id];
      return next;
    });
    updateDesktop((current) => ({
      ...current,
      projects: current.projects.map((project) =>
        project.id !== activeProject.id
          ? project
          : {
              ...project,
              conversations: project.conversations.map((conversation) =>
                conversation.id !== activeConversation.id
                  ? conversation
                  : {
                      ...conversation,
                      title,
                      modelId: selectedModel.id,
                      updatedAt: Date.now(),
                      messages: [
                        ...conversation.messages,
                        userMessage,
                        assistantMessage,
                      ],
                    },
              ),
            },
      ),
    }));
    setPrompt("");

    try {
      const result = await invoke<{ content: string }>("send_message", {
        conversationId: activeConversation.id,
        request: {
          model: selectedModel.id,
          protocol: selectedModel.protocol,
          effort,
          permissionMode,
          projectPath: activeProject.path,
          messages: history,
        },
      });
      setConversationResult(
        activeProject.id,
        activeConversation.id,
        assistantMessage.id,
        "done",
        result.content,
      );
    } catch (error) {
      const message = conciseError(error);
      setConversationResult(
        activeProject.id,
        activeConversation.id,
        assistantMessage.id,
        "error",
        `请求失败：${message}`,
      );
    }
  };

  const handleSubmit = (event: FormEvent): void => {
    event.preventDefault();
    void sendPrompt();
  };

  const handlePromptKeyDown = (
    event: KeyboardEvent<HTMLTextAreaElement>,
  ): void => {
    if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) {
      event.preventDefault();
      void sendPrompt();
    }
  };

  const copyMessage = async (message: ChatMessage): Promise<void> => {
    await navigator.clipboard.writeText(message.content);
    setCopiedMessage(message.id);
    window.setTimeout(() => setCopiedMessage(undefined), 1400);
  };

  const resolveApproval = async (
    interaction: AgentInteraction,
    decision: "approved" | "rejected",
    session = false,
  ): Promise<void> => {
    if (!activeConversation || resolvingInteraction) return;
    setResolvingInteraction(interaction.id);
    try {
      await invoke("respond_interaction", {
        conversationId: activeConversation.id,
        interactionId: interaction.id,
        response: {
          decision,
          ...(session ? { scope: "session" } : {}),
          selectedLabel:
            decision === "rejected"
              ? "Reject"
              : session
                ? "Approve for this session"
                : "Approve once",
        },
      });
    } catch (error) {
      showNotice(conciseError(error));
    } finally {
      setResolvingInteraction(undefined);
    }
  };

  return (
    <div className="app-shell">
      <aside className={sidebarCollapsed ? "sidebar collapsed" : "sidebar"}>
        <div className="brand-row">
          <div className="brand-mark" aria-label="Kimi Code">
            <span />
            <span />
          </div>
          {!sidebarCollapsed && (
            <>
              <div className="brand-copy">
                <strong>Kimi Code</strong>
                <span>Agent Desktop</span>
              </div>
              <button
                className="icon-button quiet"
                onClick={() => setSidebarCollapsed(true)}
                title="收起侧栏"
              >
                <PanelLeftClose size={17} />
              </button>
            </>
          )}
          {sidebarCollapsed && (
            <button
              className="icon-button quiet"
              onClick={() => setSidebarCollapsed(false)}
              title="展开侧栏"
            >
              <PanelLeftOpen size={17} />
            </button>
          )}
        </div>

        <div className="sidebar-primary">
          <button className="new-project-button" onClick={() => void addProject()}>
            <Plus size={17} />
            {!sidebarCollapsed && <span>打开项目</span>}
          </button>

          {!sidebarCollapsed && (
            <div className="sidebar-section-heading">
              <span>项目</span>
            </div>
          )}

          <nav className="project-list" aria-label="项目和对话">
            {desktop.projects.map((project) => {
              const isProjectActive = project.id === activeProject?.id;
              return (
                <div
                  className={`project-group ${isProjectActive ? "active" : ""}`}
                  key={project.id}
                >
                  <div
                    className="project-row"
                    onClick={() =>
                      sidebarCollapsed
                        ? setSidebarCollapsed(false)
                        : toggleProject(project.id)
                    }
                    title={project.path}
                  >
                    <span
                      className="project-glyph"
                      style={{ "--project-accent": project.accent } as React.CSSProperties}
                    >
                      <FolderGit2 size={16} />
                    </span>
                    {!sidebarCollapsed && (
                      <>
                        <span className="project-name">{project.name}</span>
                        <span className="project-actions">
                          <button
                            className="icon-button tiny"
                            onClick={(event) => createConversation(project, event)}
                            title="新建对话"
                          >
                            <Plus size={14} />
                          </button>
                          {project.expanded ? (
                            <ChevronDown size={14} />
                          ) : (
                            <ChevronRight size={14} />
                          )}
                        </span>
                      </>
                    )}
                  </div>
                  {!sidebarCollapsed && project.expanded && (
                    <div className="conversation-list">
                      {project.conversations.map((conversation) => (
                        <button
                          className={`conversation-row ${
                            conversation.id === activeConversation?.id
                              ? "selected"
                              : ""
                          }`}
                          key={conversation.id}
                          onClick={() =>
                            selectConversation(project.id, conversation.id)
                          }
                        >
                          <MessageSquareText size={14} />
                          <span>{conversation.title}</span>
                          <time>{formatTime(conversation.updatedAt)}</time>
                        </button>
                      ))}
                    </div>
                  )}
                </div>
              );
            })}
          </nav>

          {!sidebarCollapsed && desktop.projects.length === 0 && (
            <div className="sidebar-empty">
              <Folder size={22} />
              <p>打开一个本地目录，开始和 Kimi 一起写代码。</p>
            </div>
          )}
        </div>

        <div className="account-area">
          {auth.loggedIn ? (
            <div className="profile-wrap">
              <button
                className="account-button"
                onClick={() => setProfileOpen((value) => !value)}
              >
                <span className="avatar">
                  <Sparkles size={15} />
                  <i />
                </span>
                {!sidebarCollapsed && (
                  <>
                    <span className="account-copy">
                      <strong>Kimi Code</strong>
                      <small>已连接</small>
                    </span>
                    <MoreHorizontal size={16} />
                  </>
                )}
              </button>
              {profileOpen && (
                <div className="profile-popover">
                  <div>
                    <strong>Kimi Code</strong>
                    <span>OAuth 账号</span>
                  </div>
                  <button onClick={() => void signOut()}>
                    <LogOut size={15} />
                    退出登录
                  </button>
                </div>
              )}
            </div>
          ) : (
            <button className="account-button login" onClick={startLogin}>
              <span className="avatar signed-out">
                <CircleUserRound size={18} />
              </span>
              {!sidebarCollapsed && (
                <>
                  <span className="account-copy">
                    <strong>登录 Kimi</strong>
                    <small>同步模型与额度</small>
                  </span>
                  <LogIn size={16} />
                </>
              )}
            </button>
          )}
        </div>
      </aside>

      <main className="workspace">
        {activeProject && activeConversation ? (
          <>
            <header className="chat-header">
              <div className="chat-heading">
                {sidebarCollapsed && (
                  <button
                    className="icon-button"
                    onClick={() => setSidebarCollapsed(false)}
                  >
                    <Menu size={18} />
                  </button>
                )}
                <div>
                  <h1>{activeConversation.title}</h1>
                  <div className="path-line">
                    <Folder size={12} />
                    <span>{activeProject.path}</span>
                  </div>
                </div>
              </div>
              <div className="header-actions">
                <span className="connection-pill">
                  <i className={auth.loggedIn ? "online" : ""} />
                  {auth.loggedIn ? "Core v2 已连接" : "等待登录"}
                </span>
                <button className="icon-button" title="新建对话" onClick={() => createConversation(activeProject)}>
                  <SquarePen size={17} />
                </button>
              </div>
            </header>

            <div className="chat-scroll" ref={scrollRef}>
              {activeConversation.messages.length === 0 ? (
                <Welcome
                  project={activeProject}
                  onSuggestion={(value) => void sendPrompt(value)}
                />
              ) : (
                <div className="message-stack">
                  {activeConversation.messages.map((message) => (
                    <MessageView
                      key={message.id}
                      message={message}
                      copied={copiedMessage === message.id}
                      onCopy={() => void copyMessage(message)}
                    />
                  ))}
                  {activeCompaction && (
                    <CompactionNotice event={activeCompaction} />
                  )}
                </div>
              )}
            </div>

            <div className="composer-dock">
              {activeApproval && (
                <ApprovalCard
                  interaction={activeApproval}
                  busy={resolvingInteraction === activeApproval.id}
                  onReject={() =>
                    void resolveApproval(activeApproval, "rejected")
                  }
                  onApprove={() =>
                    void resolveApproval(activeApproval, "approved")
                  }
                  onApproveSession={() =>
                    void resolveApproval(activeApproval, "approved", true)
                  }
                />
              )}
              <form className="composer" onSubmit={handleSubmit}>
                <textarea
                  ref={textareaRef}
                  value={prompt}
                  onChange={(event) => setPrompt(event.target.value)}
                  onKeyDown={handlePromptKeyDown}
                  placeholder={
                    auth.loggedIn
                      ? "告诉 Kimi 你想完成什么…"
                      : "登录后开始与 Kimi Code 对话…"
                  }
                  rows={1}
                  disabled={isStreaming}
                />
                <div className="composer-toolbar">
                  <div className="composer-options">
                    <ToolbarSelect
                      className="model-select"
                      ariaLabel="选择模型"
                      icon={<Bot size={15} />}
                      value={selectedModel?.id ?? ""}
                      label={
                        modelsBusy
                          ? "同步模型中…"
                          : selectedModel?.displayName ??
                            (auth.loggedIn ? "暂无模型" : "登录后选择模型")
                      }
                      disabled={modelsBusy || !models.length}
                      options={models.map((model) => ({
                        value: model.id,
                        label: model.displayName,
                        description: `${formatContext(model.contextLength)} 上下文${
                          model.supportsReasoning ? " · 支持深度思考" : ""
                        }`,
                      }))}
                      onChange={chooseModel}
                    />
                    {auth.loggedIn && (
                      <button
                        className="toolbar-icon"
                        type="button"
                        title="刷新模型列表"
                        onClick={() => void loadModels()}
                        disabled={modelsBusy}
                      >
                        <RefreshCw size={14} />
                      </button>
                    )}
                    {selectedModel?.supportsReasoning && (
                      <ToolbarSelect
                        className="effort-select"
                        ariaLabel="选择思考强度"
                        icon={<BrainCircuit size={15} />}
                        value={effort}
                        label={`思考 · ${effort}`}
                        options={(selectedModel.supportEfforts.length
                          ? selectedModel.supportEfforts
                          : ["low", "medium", "high"]
                        ).map((value) => ({
                          value,
                          label: `思考 · ${value}`,
                          description:
                            value === "low"
                              ? "快速响应，适合简单任务"
                              : value === "high"
                                ? "更深入分析复杂问题"
                                : "速度与推理深度平衡",
                        }))}
                        onChange={setEffort}
                      />
                    )}
                    <ToolbarSelect
                      className={`permission-select ${
                        permissionMode === "yolo"
                          ? "full-access"
                          : permissionMode === "auto"
                            ? "auto-access"
                            : ""
                      }`}
                      ariaLabel="选择权限模式"
                      icon={<ShieldCheck size={15} />}
                      value={permissionMode}
                      label={
                        permissionMode === "yolo"
                          ? "完全访问"
                          : permissionMode === "auto"
                            ? "自动选择"
                            : "请求审批"
                      }
                      disabled={isStreaming}
                      options={[
                        {
                          value: "manual",
                          label: "请求审批",
                          description: "执行命令前由你确认",
                        },
                        {
                          value: "auto",
                          label: "自动选择",
                          description: "由权限策略判断是否允许",
                        },
                        {
                          value: "yolo",
                          label: "完全访问",
                          description: "跳过审批并直接执行命令",
                          danger: true,
                        },
                      ]}
                      onChange={(value) =>
                        choosePermissionMode(value as PermissionMode)
                      }
                    />
                    {selectedModel && (
                      <span className="context-badge">
                        {formatContext(selectedModel.contextLength)} 上下文
                      </span>
                    )}
                  </div>
                  <div className="send-zone">
                    <span>Enter 发送</span>
                    <button
                      className="send-button"
                      type="submit"
                      disabled={!prompt.trim() || isStreaming}
                      title="发送"
                    >
                      {isStreaming ? <span className="send-loader" /> : <ArrowUp size={18} />}
                    </button>
                  </div>
                </div>
              </form>
              <p className="composer-caption">
                Kimi 可能会犯错，请检查生成的代码和重要信息。
              </p>
            </div>
          </>
        ) : (
          <ProjectLanding
            collapsed={sidebarCollapsed}
            onExpand={() => setSidebarCollapsed(false)}
            onAddProject={() => void addProject()}
          />
        )}
      </main>

      {loginOpen && (
        <LoginDialog
          busy={loginBusy}
          code={deviceCode}
          onClose={() => !loginBusy && setLoginOpen(false)}
          onStart={() => void startLogin()}
        />
      )}

      {notice && (
        <div className="toast" role="status">
          <span>{notice}</span>
          <button aria-label="关闭提示" onClick={() => setNotice(undefined)}>
            <X size={14} />
          </button>
        </div>
      )}
    </div>
  );
}

interface ToolbarSelectOption {
  value: string;
  label: string;
  description?: string;
  danger?: boolean;
}

function CompactionNotice({ event }: { event: CompactionEvent }) {
  const completed = event.phase === "completed";
  const cancelled = event.phase === "cancelled";
  const detail = completed
    ? event.tokensBefore !== undefined && event.tokensAfter !== undefined
      ? `${formatContext(Math.round(event.tokensBefore))} → ${formatContext(
          Math.round(event.tokensAfter),
        )} tokens${
          event.compactedCount !== undefined
            ? ` · 整理 ${Math.round(event.compactedCount)} 条上下文`
            : ""
        }`
      : "较早的对话已整理为上下文摘要"
    : cancelled
      ? "本次上下文整理未完成，对话内容保持不变"
      : `${
          event.trigger === "auto" ? "自动触发" : "手动触发"
        } · 正在将较早的对话整理为摘要`;

  return (
    <div className={`compaction-notice ${event.phase}`} role="status">
      <span className="compaction-glyph">
        {completed ? (
          <Check size={14} />
        ) : cancelled ? (
          <X size={14} />
        ) : (
          <Minimize2 size={14} />
        )}
      </span>
      <span>
        <strong>
          {completed
            ? "上下文压缩完成"
            : cancelled
              ? "上下文压缩已取消"
              : "正在压缩上下文"}
        </strong>
        <small>{detail}</small>
      </span>
      {event.phase === "started" && <i />}
    </div>
  );
}

function ToolbarSelect({
  className = "",
  ariaLabel,
  icon,
  value,
  label,
  options,
  disabled = false,
  onChange,
}: {
  className?: string;
  ariaLabel: string;
  icon: ReactNode;
  value: string;
  label: string;
  options: ToolbarSelectOption[];
  disabled?: boolean;
  onChange: (value: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const close = (event: PointerEvent): void => {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false);
    };
    document.addEventListener("pointerdown", close);
    return () => document.removeEventListener("pointerdown", close);
  }, [open]);

  return (
    <div
      className={`toolbar-select ${className} ${open ? "open" : ""}`}
      ref={rootRef}
      onKeyDown={(event) => {
        if (event.key === "Escape") setOpen(false);
      }}
    >
      <button
        type="button"
        className="toolbar-select-trigger"
        aria-label={ariaLabel}
        aria-haspopup="listbox"
        aria-expanded={open}
        disabled={disabled}
        onClick={() => setOpen((current) => !current)}
      >
        {icon}
        <span>{label}</span>
        <ChevronDown size={13} />
      </button>
      {open && (
        <div className="toolbar-select-menu" role="listbox" aria-label={ariaLabel}>
          {options.map((option) => {
            const selected = option.value === value;
            return (
              <button
                type="button"
                role="option"
                aria-selected={selected}
                className={`${selected ? "selected" : ""} ${
                  option.danger ? "danger" : ""
                }`}
                key={option.value}
                onClick={() => {
                  onChange(option.value);
                  setOpen(false);
                }}
              >
                <span>
                  <strong>{option.label}</strong>
                  {option.description && <small>{option.description}</small>}
                </span>
                {selected && <Check size={14} />}
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}

function ApprovalCard({
  interaction,
  busy,
  onReject,
  onApprove,
  onApproveSession,
}: {
  interaction: AgentInteraction;
  busy: boolean;
  onReject: () => void;
  onApprove: () => void;
  onApproveSession: () => void;
}) {
  const payload = interaction.payload as ApprovalPayload;
  const display = payload.display;
  const isCommand = display?.kind === "command" && "command" in display;
  const command = isCommand ? String(display.command) : undefined;
  const cwd = isCommand && display.cwd ? String(display.cwd) : undefined;
  const detail =
    !isCommand && display
      ? ("path" in display && display.path) ||
        ("summary" in display && display.summary) ||
        payload.action
      : undefined;

  return (
    <section className="approval-card" aria-live="polite">
      <div className="approval-icon">
        <ShieldAlert size={19} />
      </div>
      <div className="approval-content">
        <div className="approval-heading">
          <div>
            <span>需要你的批准</span>
            <strong>{payload.action || `${payload.toolName} 请求执行操作`}</strong>
          </div>
          <span className="approval-tool">{payload.toolName}</span>
        </div>
        {command ? (
          <div className="approval-command">
            <div>
              <TerminalSquare size={13} />
              <span>{cwd || "当前项目目录"}</span>
            </div>
            <code>{command}</code>
          </div>
        ) : (
          <div className="approval-detail">{String(detail || "该操作需要确认")}</div>
        )}
        <div className="approval-footer">
          <p>请确认命令及工作目录可信后再允许执行。</p>
          <div className="approval-actions">
            <button type="button" className="approval-reject" onClick={onReject} disabled={busy}>
              拒绝
            </button>
            <button type="button" className="approval-session" onClick={onApproveSession} disabled={busy}>
              本会话允许
            </button>
            <button type="button" className="approval-once" onClick={onApprove} disabled={busy}>
              {busy ? <span className="spinner light" /> : <Check size={14} />}
              允许一次
            </button>
          </div>
        </div>
      </div>
    </section>
  );
}

function Welcome({
  project,
  onSuggestion,
}: {
  project: Project;
  onSuggestion: (value: string) => void;
}) {
  return (
    <section className="welcome">
      <div className="welcome-orbit">
        <span className="orbit orbit-one" />
        <span className="orbit orbit-two" />
        <div className="welcome-mark">
          <Code2 size={27} />
        </div>
      </div>
      <p className="eyebrow">KIMI CODE AGENT</p>
      <h2>
        准备好一起构建
        <br />
        <span>{project.name}</span> 了吗？
      </h2>
      <p className="welcome-copy">
        我会结合当前项目上下文理解你的目标。你可以让我阅读代码、解释结构，或从一个具体任务开始。
      </p>
      <div className="suggestion-grid">
        {PROMPT_SUGGESTIONS.map((suggestion) => (
          <button
            key={suggestion.title}
            onClick={() => onSuggestion(suggestion.prompt)}
          >
            <span>{suggestion.icon}</span>
            <strong>{suggestion.title}</strong>
            <small>{suggestion.prompt}</small>
            <ArrowUp size={15} />
          </button>
        ))}
      </div>
    </section>
  );
}

function MessageView({
  message,
  copied,
  onCopy,
}: {
  message: ChatMessage;
  copied: boolean;
  onCopy: () => void;
}) {
  const [thinkingOpen, setThinkingOpen] = useState(false);
  if (message.role === "user") {
    return (
      <article className="message user-message">
        <div className="message-meta">
          <span>你</span>
          <time>{formatTime(message.createdAt)}</time>
        </div>
        <div className="user-bubble">{message.content}</div>
      </article>
    );
  }
  return (
    <article className={`message assistant-message ${message.status ?? ""}`}>
      <div className="assistant-rail">
        <span className="assistant-avatar">
          <Sparkles size={15} />
        </span>
        <i />
      </div>
      <div className="assistant-body">
        <div className="message-meta">
          <span>Kimi</span>
          <time>{formatTime(message.createdAt)}</time>
        </div>
        {message.thinking && (
          <div className="thinking-block">
            <button onClick={() => setThinkingOpen((value) => !value)}>
              <BrainCircuit size={14} />
              <span>思考过程</span>
              {thinkingOpen ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
            </button>
            {thinkingOpen && <p>{message.thinking}</p>}
          </div>
        )}
        <div className="markdown-body">
          {message.content ? (
            <MarkdownMessage content={message.content} />
          ) : (
            <div className="typing">
              <i />
              <i />
              <i />
            </div>
          )}
        </div>
        {message.status !== "streaming" && message.content && (
          <div className="message-actions">
            <button onClick={onCopy}>
              {copied ? <Check size={14} /> : <Copy size={14} />}
              {copied ? "已复制" : "复制"}
            </button>
          </div>
        )}
      </div>
    </article>
  );
}

function MarkdownCodeBlock({ children }: { children: ReactNode }) {
  const className = isValidElement<{ className?: string }>(children)
    ? children.props.className
    : undefined;
  const language = className?.match(/language-([^\s]+)/)?.[1] ?? "code";

  return (
    <div className="code-wrap">
      <div className="code-label">
        <span>{language}</span>
      </div>
      <pre>{children}</pre>
    </div>
  );
}

function MarkdownMessage({ content }: { content: string }) {
  return (
    <ReactMarkdown
      remarkPlugins={[remarkGfm]}
      components={{
        pre({ children }) {
          return <MarkdownCodeBlock>{children}</MarkdownCodeBlock>;
        },
        code({ className, children, ...props }) {
          return (
            <code className={className} {...props}>
              {children}
            </code>
          );
        },
        table({ children }) {
          return (
            <div className="markdown-table-wrap">
              <table>{children}</table>
            </div>
          );
        },
        a({ children, ...props }) {
          return (
            <a {...props} target="_blank" rel="noreferrer">
              {children}
            </a>
          );
        },
      }}
    >
      {content}
    </ReactMarkdown>
  );
}

function ProjectLanding({
  collapsed,
  onExpand,
  onAddProject,
}: {
  collapsed: boolean;
  onExpand: () => void;
  onAddProject: () => void;
}) {
  return (
    <div className="project-landing">
      {collapsed && (
        <button className="landing-menu icon-button" onClick={onExpand}>
          <Menu size={18} />
        </button>
      )}
      <div className="landing-visual">
        <span className="landing-grid" />
        <div className="landing-folder">
          <FolderGit2 size={42} />
        </div>
        <i className="landing-dot dot-one" />
        <i className="landing-dot dot-two" />
        <i className="landing-dot dot-three" />
      </div>
      <p className="eyebrow">YOUR AI CODING PARTNER</p>
      <h1>从一个项目开始</h1>
      <p>
        选择本地代码目录。每个项目都有独立的对话空间，
        <br />
        你的上下文和灵感会一直留在这里。
      </p>
      <button className="landing-primary" onClick={onAddProject}>
        <Folder size={17} />
        打开本地项目
      </button>
      <div className="landing-shortcut">
        <span>提示</span>
        你也可以把项目文件夹拖到窗口中
      </div>
    </div>
  );
}

function LoginDialog({
  busy,
  code,
  onClose,
  onStart,
}: {
  busy: boolean;
  code?: DeviceCode;
  onClose: () => void;
  onStart: () => void;
}) {
  const [copied, setCopied] = useState(false);
  const copyCode = async (): Promise<void> => {
    if (!code) return;
    await navigator.clipboard.writeText(code.userCode);
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1500);
  };
  return (
    <div className="modal-backdrop" onMouseDown={onClose}>
      <section className="login-dialog" onMouseDown={(event) => event.stopPropagation()}>
        <button
          className="dialog-close"
          aria-label="关闭登录窗口"
          onClick={onClose}
          disabled={busy}
        >
          <X size={17} />
        </button>
        <div className="login-logo">
          <Sparkles size={24} />
        </div>
        <p className="eyebrow">KIMI CODE ACCOUNT</p>
        <h2>连接你的 Kimi 账号</h2>
        <p className="dialog-copy">
          登录后会安全地同步可用模型。授权信息由 agent-core-v2 保存在本机。
        </p>
        {code ? (
          <>
            <button className="device-code" onClick={() => void copyCode()}>
              <span>设备验证码</span>
              <strong>{code.userCode}</strong>
              <small>{copied ? "已复制" : "点击复制"}</small>
            </button>
            <button
              className="dialog-primary"
              onClick={() => void openUrl(code.verificationUriComplete || code.verificationUri)}
            >
              在浏览器中授权
              <ExternalLink size={16} />
            </button>
            <div className="waiting-line">
              <span className="spinner" />
              等待浏览器确认…
            </div>
          </>
        ) : (
          <>
            <div className="login-features">
              <span><Check size={14} /> OAuth 安全登录</span>
              <span><Check size={14} /> 自动同步模型</span>
              <span><Check size={14} /> 凭证仅保存在本机</span>
            </div>
            <button className="dialog-primary" onClick={onStart} disabled={busy}>
              {busy ? (
                <>
                  <span className="spinner light" />
                  正在创建授权…
                </>
              ) : (
                <>
                  继续登录
                  <ArrowUp size={16} />
                </>
              )}
            </button>
          </>
        )}
      </section>
    </div>
  );
}

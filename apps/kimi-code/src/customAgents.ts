import { getLanguage } from "./i18n.ts";

export type CustomAgentScope = "app" | "project";

export interface CustomAgentDescriptor {
  scope: CustomAgentScope;
  relativePath: string;
  path: string;
  content: string;
  name: string;
  description?: string;
  whenToUse?: string;
  isOverride: boolean;
  tools?: string[];
  disallowedTools?: string[];
  subagents?: string[];
  model?: string;
  valid: boolean;
  error?: string;
}

export interface SaveCustomAgentInput {
  workspaceId: string;
  scope: CustomAgentScope;
  relativePath?: string;
  content: string;
}

export interface DeleteCustomAgentInput {
  workspaceId: string;
  scope: CustomAgentScope;
  relativePath: string;
}

const ZH_TEMPLATE = `---
# 名称使用 kebab-case；新建时也会作为文件名。
name: custom-agent
description: 简洁说明这个子代理擅长什么
whenToUse: 说明主代理应在什么情况下调用它

# 仅覆盖同名内置代理时设为 true。
override: false

# 留空表示继承调用者当前模型，也可以填写已配置的模型别名。
model: 

# 可用 '*' 表示不额外限制；下面是安全的只读起点。
tools:
  - Read
  - Glob
  - Grep
disallowedTools:
  - Write
  - Edit
  - Bash

# 可用 '*' 允许调用任意子代理，或列出明确的类型。
subagents:
  - explore
---

\${base_prompt}

你是一个专注于特定任务的子代理。请在这里完整描述你的职责、工作方式和边界。

工作要求：
- 先理解父代理交付的目标和上下文。
- 严格遵守工具权限与任务边界。
- 最终向父代理返回清晰、具体、可执行的结论。
`;

const EN_TEMPLATE = `---
# Use a kebab-case name. New agents also use it as the file name.
name: custom-agent
description: Briefly describe what this subagent does best
whenToUse: Explain when the parent agent should delegate to it

# Set to true only when replacing a same-name built-in profile.
override: false

# Leave null to inherit the caller's model, or use a configured model alias.
model: 

# Use '*' for no extra restriction. This is a safe read-only starting point.
tools:
  - Read
  - Glob
  - Grep
disallowedTools:
  - Write
  - Edit
  - Bash

# Use '*' to allow any subagent, or list explicit profile names.
subagents:
  - explore
---

\${base_prompt}

You are a subagent focused on a specific kind of work. Fully describe your responsibilities, working method, and boundaries here.

Requirements:
- Understand the goal and context handed off by the parent agent.
- Stay within the configured tool permissions and task boundaries.
- Return a clear, concrete, actionable final handoff to the parent agent.
`;

export function newCustomAgentTemplate(): string {
  return getLanguage() === "zh" ? ZH_TEMPLATE : EN_TEMPLATE;
}

export function customAgentKey(
  agent: Pick<CustomAgentDescriptor, "scope" | "relativePath">,
): string {
  return `${agent.scope}:${agent.relativePath}`;
}

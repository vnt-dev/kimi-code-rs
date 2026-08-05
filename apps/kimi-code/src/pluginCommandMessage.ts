export interface PluginCommandDisplay {
  pluginId: string;
  commandName: string;
  args?: string;
}

export interface PluginCommandDetail extends PluginCommandDisplay {
  id: string;
  content: string;
  createdAt: string;
}

export function pluginCommandFromOrigin(
  origin: unknown,
): PluginCommandDisplay | undefined {
  if (!origin || typeof origin !== "object") return undefined;
  const value = origin as Record<string, unknown>;
  if (
    value.kind !== "plugin_command" ||
    value.trigger !== "user-slash" ||
    typeof value.pluginId !== "string" ||
    typeof value.commandName !== "string"
  ) {
    return undefined;
  }
  return {
    pluginId: value.pluginId,
    commandName: value.commandName,
    args: typeof value.commandArgs === "string" ? value.commandArgs : undefined,
  };
}

export function pluginCommandText(command: PluginCommandDisplay): string {
  const name = `/${command.pluginId}:${command.commandName}`;
  return command.args ? `${name} ${command.args}` : name;
}

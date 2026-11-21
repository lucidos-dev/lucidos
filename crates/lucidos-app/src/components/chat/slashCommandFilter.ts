/** What the `/` menu's filter box holds: a command name, then its arguments.
 *  A space ends the name, as it does in Claude Code's own prompt. */
export interface SlashFilter {
  /** The name typed so far, lowercased. */
  query: string;
  /** Everything after the first space, trimmed. */
  args: string;
  /** True once a space follows the name, so the name must match exactly. */
  nameEnded: boolean;
}

export function parseSlashFilter(filter: string): SlashFilter {
  const text = filter.trimStart();
  const space = text.search(/\s/);
  if (space < 0) return { query: text.toLowerCase(), args: '', nameEnded: false };
  return { query: text.slice(0, space).toLowerCase(), args: text.slice(space).trim(), nameEnded: true };
}

/** The commands the filter shows: a partial match while the name is typed,
 *  the exact name once arguments follow. */
export function filterSlashCommands(commands: string[], filter: string): string[] {
  const { query, nameEnded } = parseSlashFilter(filter);
  if (!query) return commands;
  return commands.filter((name) => {
    const lower = name.toLowerCase();
    return nameEnded ? lower === query : lower.includes(query);
  });
}

/** The message a picked command sends: the command, then the arguments typed
 *  after its name in the filter. */
export function slashCommandText(name: string, filter: string): string {
  const { args } = parseSlashFilter(filter);
  return args ? `/${name} ${args}` : `/${name}`;
}

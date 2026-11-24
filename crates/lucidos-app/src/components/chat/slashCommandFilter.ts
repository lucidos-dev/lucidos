export interface SlashFilter {
  query: string;
  args: string;
  nameEnded: boolean;
}

export function parseSlashFilter(filter: string): SlashFilter {
  const text = filter.trimStart();
  const space = text.search(/\s/);
  if (space < 0) return { query: text.toLowerCase(), args: '', nameEnded: false };
  return { query: text.slice(0, space).toLowerCase(), args: text.slice(space).trim(), nameEnded: true };
}

export function filterSlashCommands(commands: string[], filter: string): string[] {
  const { query, nameEnded } = parseSlashFilter(filter);
  if (!query) return commands;
  return commands.filter((name) => {
    const lower = name.toLowerCase();
    return nameEnded ? lower === query : lower.includes(query);
  });
}

export function slashCommandText(name: string, filter: string): string {
  const { args } = parseSlashFilter(filter);
  return args ? `/${name} ${args}` : `/${name}`;
}

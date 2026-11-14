import { Fragment } from 'preact';
import { SkText } from '../shared/Skeleton';

export function splitPluginDescription(description: string): string[] {
  return description.replace(/\r\n/g, '\n').split(/\n[^\S\n]*\n/)
    .map((part) => part.trim().replace(/[^\S\n]*\n[^\S\n]*/g, ' '))
    .filter(Boolean);
}

export function PluginDescription({ description = '', class: cls, w }: {
  description?: string;
  class: string;
  w: string;
}) {
  return (
    <SkText class={cls} as="div" w={w}>
      {splitPluginDescription(description).map((paragraph, i) => (
        <Fragment key={i}>{i > 0 && '\n'}<p>{paragraph}</p></Fragment>
      ))}
    </SkText>
  );
}

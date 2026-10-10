Some ordinary read-only shell commands that the Lucidos agent runs no longer
get approved at once. They now wait for the slower safety judge, which makes
simple chats sluggish. Examples we saw:

- `ENVIRONMENT=staging cat data/config.yaml`
- `ENV_FILE=.env ls`
- `IFS_MODE=x echo hi`
- `grep NODE_OPTIONS= data/f.txt`
- `echo LD_PRELOAD=x`

`FOO=1 ls` is still approved at once. A command that really sets a
code-loading variable, such as `LD_PRELOAD=/tmp/evil.so ls`, should of course
keep going to the judge.

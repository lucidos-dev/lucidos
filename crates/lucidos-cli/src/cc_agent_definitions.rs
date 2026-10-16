//! Resolves an `Agent` call's `subagent_type` to the definition Claude Code
//! would launch, for `cc_agent_guard`. It follows Claude Code 2.1.280: the
//! same directories, the same precedence and the same name matching.
//!
//! Every read or parse error skips that file or directory, so a definition
//! the guard cannot read lets the call through.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;
use unicode_normalization::UnicodeNormalization;

/// What the guard needs to know about one agent definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentDefinition {
    pub(crate) agent_type: String,
    /// `background: true`: Claude Code runs it in the background whatever the
    /// call says, unless background tasks are disabled.
    pub(crate) background: bool,
    /// `isolation: remote`, which applies when the call sets no isolation.
    pub(crate) remote_isolation: bool,
}

impl AgentDefinition {
    fn built_in(agent_type: &str) -> Self {
        AgentDefinition {
            agent_type: agent_type.to_string(),
            background: false,
            remote_isolation: false,
        }
    }
}

/// Claude Code's built-in agents, none of which sets `background`. A custom
/// agent of the same name replaces one. Listing them also stops a request for
/// a built-in from loosely matching a custom agent.
const BUILT_IN_AGENT_TYPES: [&str; 7] = [
    "general-purpose",
    "Explore",
    "Plan",
    "statusline-setup",
    "claude",
    "claude-code-guide",
    "web-fetch",
];

/// The type Claude Code launches when the call names none.
pub(crate) const DEFAULT_AGENT_TYPE: &str = "general-purpose";

/// The session's `--add-dir` list, a JSON array the engine exports
/// (`runtime::claude_code::CC_ADDITIONAL_DIRECTORIES_ENV`).
const ADDITIONAL_DIRECTORIES_ENV: &str = "LUCIDOS_CC_ADDITIONAL_DIRECTORIES";

#[cfg(target_os = "macos")]
const MANAGED_DIR: &str = "/Library/Application Support/ClaudeCode";
#[cfg(not(target_os = "macos"))]
const MANAGED_DIR: &str = "/etc/claude-code";

/// The places Claude Code reads agent definitions from.
pub(crate) struct AgentSources {
    /// The session's working directory, from the hook payload.
    pub(crate) cwd: PathBuf,
    /// The project walk stops below it.
    pub(crate) home: Option<PathBuf>,
    /// `$CLAUDE_CONFIG_DIR`, or `~/.claude`.
    pub(crate) config_dir: Option<PathBuf>,
    /// The managed-policy directory.
    pub(crate) managed_dir: PathBuf,
    /// The session's `--add-dir` directories, in order.
    pub(crate) additional_dirs: Vec<PathBuf>,
}

impl AgentSources {
    pub(crate) fn from_env(cwd: PathBuf) -> Self {
        let home = dirs::home_dir();
        let config_dir = std::env::var_os("CLAUDE_CONFIG_DIR")
            .filter(|dir| !dir.is_empty())
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|home| home.join(".claude")));
        AgentSources {
            cwd,
            home,
            config_dir,
            managed_dir: PathBuf::from(MANAGED_DIR),
            additional_dirs: parse_additional_dirs(
                std::env::var(ADDITIONAL_DIRECTORIES_ENV).ok().as_deref(),
            ),
        }
    }

    /// Only plugin agent types contain `:`, and Claude Code refuses a `:` in
    /// any other agent's name. So one source set can answer each request.
    fn candidates(&self, subagent_type: &str) -> Vec<AgentDefinition> {
        if subagent_type.contains(':') {
            self.plugin_agents()
        } else {
            self.non_plugin_agents()
        }
    }

    /// Lowest precedence first: built-in, user, additional directories,
    /// project, managed policy.
    fn non_plugin_agents(&self) -> Vec<AgentDefinition> {
        let mut agents: Vec<_> = BUILT_IN_AGENT_TYPES
            .iter()
            .map(|agent_type| AgentDefinition::built_in(agent_type))
            .collect();
        if let Some(config_dir) = &self.config_dir {
            agents.extend(directory_agents(&config_dir.join("agents")));
        }
        for dir in &self.additional_dirs {
            agents.extend(directory_agents(&dir.join(".claude/agents")));
        }
        for dir in self.project_agent_dirs() {
            agents.extend(directory_agents(&dir));
        }
        agents.extend(directory_agents(&self.managed_dir.join(".claude/agents")));
        agents
    }

    /// `.claude/agents` in the working directory and each parent, up to the
    /// git root and never including home. A deeper directory wins. A linked
    /// worktree with no agents directory of its own reads its main checkout's.
    fn project_agent_dirs(&self) -> Vec<PathBuf> {
        let git_root = git_root(&self.cwd);
        let mut dirs = Vec::new();
        for dir in self.cwd.ancestors() {
            if Some(dir) == self.home.as_deref() {
                break;
            }
            dirs.push(dir.join(".claude/agents"));
            if Some(dir) == git_root {
                break;
            }
        }
        if let Some(root) = git_root {
            if !root.join(".claude/agents").is_dir() {
                if let Some(main) = main_checkout(root) {
                    dirs.push(main.join(".claude/agents"));
                }
            }
        }
        dirs.sort_by_key(|dir| dir.components().count());
        dirs
    }

    fn plugin_agents(&self) -> Vec<AgentDefinition> {
        let Some(config_dir) = &self.config_dir else {
            return Vec::new();
        };
        let installed = read_json(&config_dir.join("plugins/installed_plugins.json"));
        self.enabled_plugin_ids()
            .iter()
            .filter_map(|id| {
                let install_path = install_path(installed.as_ref()?, id, self.project_root())?;
                let fallback_name = id.split('@').next().unwrap_or(id);
                Some(plugin_agents_at(&install_path, fallback_name))
            })
            .flatten()
            .collect()
    }

    /// Where the session started. The payload's `cwd` follows the shell into a
    /// subdirectory, while Lucidos always starts Claude Code at the git root.
    fn project_root(&self) -> &Path {
        git_root(&self.cwd).unwrap_or(&self.cwd)
    }

    /// `enabledPlugins` merged over the settings files, a later file
    /// overriding an earlier one.
    fn enabled_plugin_ids(&self) -> Vec<String> {
        let mut settings_files = Vec::new();
        if let Some(config_dir) = &self.config_dir {
            settings_files.push(config_dir.join("settings.json"));
        }
        settings_files.push(self.project_root().join(".claude/settings.json"));
        settings_files.push(self.project_root().join(".claude/settings.local.json"));
        settings_files.push(self.managed_dir.join("managed-settings.json"));

        let mut enabled = HashMap::new();
        for settings in settings_files.iter().filter_map(|path| read_json(path)) {
            let Some(plugins) = settings.get("enabledPlugins").and_then(Value::as_object) else {
                continue;
            };
            for (id, on) in plugins {
                enabled.insert(id.clone(), on == &Value::Bool(true));
            }
        }
        let mut ids: Vec<_> = enabled
            .into_iter()
            .filter_map(|(id, on)| on.then_some(id))
            .collect();
        ids.sort();
        ids
    }
}

/// The directories in the engine's JSON array. Anything else yields none.
fn parse_additional_dirs(value: Option<&str>) -> Vec<PathBuf> {
    value
        .and_then(|json| serde_json::from_str::<Vec<String>>(json).ok())
        .unwrap_or_default()
        .into_iter()
        .map(PathBuf::from)
        .collect()
}

/// The definition Claude Code launches for `subagent_type`, or `None` when it
/// names no agent the guard can find.
pub(crate) fn resolve(subagent_type: &str, sources: &AgentSources) -> Option<AgentDefinition> {
    let active = active_agents(sources.candidates(subagent_type));
    if let Some(exact) = active.iter().find(|a| a.agent_type == subagent_type) {
        return Some(exact.clone());
    }
    // Claude Code launches a loose match only when exactly one agent has it.
    let wanted = loose_name(subagent_type);
    let mut loose = active
        .iter()
        .filter(|a| loose_name(&a.agent_type) == wanted);
    match (loose.next(), loose.next()) {
        (Some(only), None) => Some(only.clone()),
        _ => None,
    }
}

/// One definition per type: a later definition replaces an earlier one.
fn active_agents(agents: Vec<AgentDefinition>) -> Vec<AgentDefinition> {
    let mut by_type: HashMap<String, AgentDefinition> = HashMap::new();
    for agent in agents {
        by_type.insert(agent.agent_type.clone(), agent);
    }
    by_type.into_values().collect()
}

/// Claude Code's loose form of an agent name: NFKC, lowercase, and without
/// whitespace, underscores or dash punctuation.
fn loose_name(name: &str) -> String {
    name.nfkc()
        .filter(|&c| !(c.is_whitespace() || c == '_' || is_dash_punctuation(c)))
        .flat_map(char::to_lowercase)
        .collect()
}

/// The Unicode `Pd` category, which Claude Code strips as `\p{Pd}`.
fn is_dash_punctuation(c: char) -> bool {
    matches!(
        c,
        '-' | '\u{058A}' | '\u{05BE}' | '\u{1400}' | '\u{1806}' | '\u{2010}'
            ..='\u{2015}'
                | '\u{2E17}'
                | '\u{2E1A}'
                | '\u{2E3A}'
                | '\u{2E3B}'
                | '\u{2E40}'
                | '\u{2E5D}'
                | '\u{301C}'
                | '\u{3030}'
                | '\u{30A0}'
                | '\u{FE31}'
                | '\u{FE32}'
                | '\u{FE58}'
                | '\u{FE63}'
                | '\u{FF0D}'
                | '\u{10EAD}'
    )
}

/// The nearest directory at or above `cwd` that holds a `.git` entry.
fn git_root(cwd: &Path) -> Option<&Path> {
    cwd.ancestors().find(|dir| dir.join(".git").exists())
}

/// The main checkout of a linked worktree, whose `.git` is a file pointing at
/// `<main>/.git/worktrees/<name>`.
fn main_checkout(worktree_root: &Path) -> Option<PathBuf> {
    let dot_git = std::fs::read_to_string(worktree_root.join(".git")).ok()?;
    let git_dir = worktree_root.join(dot_git.trim().strip_prefix("gitdir:")?.trim());
    let common_dir = std::fs::read_to_string(git_dir.join("commondir")).ok()?;
    let common_dir = std::fs::canonicalize(git_dir.join(common_dir.trim())).ok()?;
    let main = common_dir.parent()?;
    (main != worktree_root).then(|| main.to_path_buf())
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// A plugin's install directory, from `installed_plugins.json`. A project or
/// local install counts only for its own project.
fn install_path(installed: &Value, id: &str, cwd: &Path) -> Option<PathBuf> {
    installed
        .get("plugins")?
        .get(id)?
        .as_array()?
        .iter()
        .find(
            |install| match install.get("projectPath").and_then(Value::as_str) {
                Some(project) => Path::new(project) == cwd,
                None => true,
            },
        )?
        .get("installPath")?
        .as_str()
        .map(PathBuf::from)
}

/// The plugin's `agents/` directory plus every path its manifest's `agents`
/// field names. Types read `<plugin>:<subdirectories>:<name>`.
fn plugin_agents_at(install_path: &Path, fallback_name: &str) -> Vec<AgentDefinition> {
    let manifest = read_json(&install_path.join(".claude-plugin/plugin.json"));
    let manifest = manifest.as_ref();
    let plugin = manifest
        .and_then(|m| m.get("name"))
        .and_then(Value::as_str)
        .unwrap_or(fallback_name);
    let extra_paths = match manifest.and_then(|m| m.get("agents")) {
        Some(Value::String(path)) => vec![path.as_str()],
        Some(Value::Array(paths)) => paths.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };

    let mut files = markdown_files(&install_path.join("agents"));
    for path in extra_paths.iter().map(|p| install_path.join(p)) {
        if path.is_dir() {
            files.extend(markdown_files(&path));
        } else if path.extension().is_some_and(|ext| ext == "md") {
            files.push(MarkdownFile {
                path,
                namespace: Vec::new(),
            });
        }
    }
    files
        .into_iter()
        .filter_map(|file| plugin_agent(plugin, &file))
        .collect()
}

fn plugin_agent(plugin: &str, file: &MarkdownFile) -> Option<AgentDefinition> {
    let frontmatter = read_frontmatter(&file.path)?;
    let name = match frontmatter.get("name") {
        Some(name) if !name.text().is_empty() => name.text().to_string(),
        _ => file.path.file_stem()?.to_str()?.to_string(),
    };
    let mut segments = vec![plugin.to_string()];
    segments.extend(file.namespace.iter().cloned());
    segments.push(name);
    Some(AgentDefinition {
        agent_type: segments.join(":"),
        background: frontmatter.get("background").is_some_and(Scalar::is_true),
        // Claude Code honours only `isolation: worktree` from a plugin agent.
        remote_isolation: false,
    })
}

/// Agents from a user, project or managed-policy directory. Claude Code skips
/// a file whose frontmatter lacks a usable `name` or a `description`.
fn directory_agents(dir: &Path) -> Vec<AgentDefinition> {
    markdown_files(dir)
        .iter()
        .filter_map(|file| {
            let frontmatter = read_frontmatter(&file.path)?;
            let name = frontmatter.get("name")?.text();
            let has_description = frontmatter
                .get("description")
                .is_some_and(|d| !d.text().is_empty());
            if name.is_empty() || name.starts_with('-') || name.contains(':') || !has_description {
                return None;
            }
            Some(AgentDefinition {
                agent_type: name.to_string(),
                background: frontmatter.get("background").is_some_and(Scalar::is_true),
                remote_isolation: frontmatter
                    .get("isolation")
                    .is_some_and(|i| i.text() == "remote"),
            })
        })
        .collect()
}

struct MarkdownFile {
    path: PathBuf,
    /// The subdirectories between the scanned root and the file.
    namespace: Vec<String>,
}

/// Bounds the walk, which follows symlinks, so a symlink loop terminates.
const MAX_DIRECTORY_DEPTH: usize = 8;

/// Every `.md` file under `root`, in name order. A missing root has none.
fn markdown_files(root: &Path) -> Vec<MarkdownFile> {
    let mut files = Vec::new();
    collect_markdown_files(root, &mut Vec::new(), &mut files);
    files
}

fn collect_markdown_files(dir: &Path, namespace: &mut Vec<String>, files: &mut Vec<MarkdownFile>) {
    if namespace.len() > MAX_DIRECTORY_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<_> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            namespace.push(name.to_string());
            collect_markdown_files(&path, namespace, files);
            namespace.pop();
        } else if path.is_file() && path.extension().is_some_and(|ext| ext == "md") {
            files.push(MarkdownFile {
                path,
                namespace: namespace.clone(),
            });
        }
    }
}

/// A top-level frontmatter value. YAML reads a quoted value as a string and
/// a plain `true` as a boolean, and Claude Code's `background` accepts both.
#[derive(Debug, PartialEq, Eq)]
enum Scalar {
    Quoted(String),
    Plain(String),
}

impl Scalar {
    fn text(&self) -> &str {
        match self {
            Scalar::Quoted(s) | Scalar::Plain(s) => s,
        }
    }

    fn is_true(&self) -> bool {
        match self {
            Scalar::Quoted(s) => s == "true",
            Scalar::Plain(s) => matches!(s.as_str(), "true" | "True" | "TRUE"),
        }
    }
}

fn read_frontmatter(path: &Path) -> Option<HashMap<String, Scalar>> {
    parse_frontmatter(&std::fs::read_to_string(path).ok()?)
}

/// The top-level `key: value` pairs between the opening and closing `---`.
/// An empty value continued on an indented line takes that line's text, as
/// YAML does. Nested structure is otherwise skipped: the guard needs only
/// `name`, `description`, `background` and `isolation`.
fn parse_frontmatter(content: &str) -> Option<HashMap<String, Scalar>> {
    let mut lines = content.trim_start_matches('\u{FEFF}').lines();
    if lines.next()?.trim_end() != "---" {
        return None;
    }
    let mut pairs = HashMap::new();
    let mut last_key: Option<String> = None;
    for line in lines {
        if line.trim_end() == "---" {
            return Some(pairs);
        }
        if line.starts_with(char::is_whitespace) {
            let continued = last_key.as_ref().and_then(|key| pairs.get_mut(key));
            if let Some(value @ Scalar::Plain(_)) = continued {
                if value.text().is_empty() && !line.trim().is_empty() {
                    *value = Scalar::Plain(line.trim().to_string());
                }
            }
            continue;
        }
        if line.starts_with(['#', '-']) {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim().to_string();
        pairs.insert(key.clone(), parse_scalar(value.trim()));
        last_key = Some(key);
    }
    None
}

fn parse_scalar(value: &str) -> Scalar {
    for quote in ['"', '\''] {
        if let Some(rest) = value.strip_prefix(quote) {
            if let Some(end) = rest.find(quote) {
                return Scalar::Quoted(rest[..end].to_string());
            }
        }
    }
    let plain = match value.find(" #") {
        Some(comment) => &value[..comment],
        None => value,
    };
    Scalar::Plain(plain.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A home with a git repo at `home/repo` and the session in its `app`
    /// subdirectory, plus separate config and managed-policy directories.
    struct Fixture {
        _tmp: tempfile::TempDir,
        root: PathBuf,
        sources: AgentSources,
    }

    impl Fixture {
        fn new() -> Self {
            let tmp = tempfile::tempdir().expect("tempdir");
            let root = tmp.path().to_path_buf();
            let repo = root.join("home/repo");
            fs::create_dir_all(repo.join(".git")).unwrap();
            fs::create_dir_all(repo.join("app")).unwrap();
            let sources = AgentSources {
                cwd: repo.join("app"),
                home: Some(root.join("home")),
                config_dir: Some(root.join("config")),
                managed_dir: root.join("managed"),
                additional_dirs: Vec::new(),
            };
            Fixture {
                _tmp: tmp,
                root,
                sources,
            }
        }

        fn write(&self, relative: &str, content: &str) {
            let path = self.root.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, content).unwrap();
        }

        fn agent(&self, relative: &str, name: &str, extra: &str) {
            self.write(
                relative,
                &format!("---\nname: {name}\ndescription: Does things.\n{extra}---\nPrompt.\n"),
            );
        }

        fn resolve(&self, subagent_type: &str) -> Option<AgentDefinition> {
            resolve(subagent_type, &self.sources)
        }

        fn background(&self, subagent_type: &str) -> bool {
            self.resolve(subagent_type).is_some_and(|a| a.background)
        }

        /// Installs and enables a plugin in the user scope.
        fn plugin(&self, id: &str, manifest: &str) -> PathBuf {
            let install = self.root.join("plugins").join(id);
            self.write(
                &format!("plugins/{id}/.claude-plugin/plugin.json"),
                manifest,
            );
            self.write(
                "config/plugins/installed_plugins.json",
                &serde_json::json!({
                    "version": 2,
                    "plugins": {id: [{"scope": "user", "installPath": install}]},
                })
                .to_string(),
            );
            self.write(
                "config/settings.json",
                &serde_json::json!({"enabledPlugins": {id: true}}).to_string(),
            );
            install
        }
    }

    #[test]
    fn a_project_agent_with_background_true_resolves_as_background() {
        let f = Fixture::new();
        f.agent(
            "home/repo/.claude/agents/scout.md",
            "scout",
            "background: true\n",
        );
        assert_eq!(
            f.resolve("scout"),
            Some(AgentDefinition {
                agent_type: "scout".into(),
                background: true,
                remote_isolation: false,
            })
        );
    }

    #[test]
    fn additional_directories_rank_between_user_and_project_agents() {
        let mut f = Fixture::new();
        f.sources.additional_dirs = vec![f.root.join("data"), f.root.join("grant")];
        f.agent("config/agents/a.md", "scout", "");
        f.agent("data/.claude/agents/a.md", "scout", "background: true\n");
        assert!(
            f.background("scout"),
            "an additional dir overrides a user agent"
        );
        f.agent("grant/.claude/agents/a.md", "scout", "");
        assert!(!f.background("scout"), "a later additional dir wins");
        f.agent("data/.claude/agents/b.md", "lead", "background: true\n");
        assert!(f.background("lead"));
        f.agent("home/repo/.claude/agents/b.md", "lead", "");
        assert!(!f.background("lead"), "a project agent overrides it");
    }

    #[test]
    fn the_additional_directories_parse_from_a_json_array() {
        assert_eq!(
            parse_additional_dirs(Some(r#"["/p/with space", "/p/with:colon"]"#)),
            [
                PathBuf::from("/p/with space"),
                PathBuf::from("/p/with:colon")
            ]
        );
        for junk in [None, Some(""), Some("/p/plain"), Some("[1, 2]"), Some("{}")] {
            assert!(parse_additional_dirs(junk).is_empty(), "{junk:?}");
        }
    }

    #[test]
    fn a_user_agent_resolves_from_the_config_dir() {
        let f = Fixture::new();
        f.agent("config/agents/scout.md", "scout", "background: true\n");
        assert!(f.background("scout"));
    }

    #[test]
    fn the_type_is_the_frontmatter_name_not_the_file_name() {
        let f = Fixture::new();
        f.agent("config/agents/file-name.md", "scout", "background: true\n");
        assert!(f.background("scout"));
        assert_eq!(f.resolve("file-name"), None);
    }

    #[test]
    fn files_in_subdirectories_count() {
        let f = Fixture::new();
        f.agent(
            "config/agents/team/deep/scout.md",
            "scout",
            "background: true\n",
        );
        assert!(f.background("scout"));
    }

    #[test]
    fn precedence_runs_user_then_shallow_project_then_deep_project_then_policy() {
        let f = Fixture::new();
        f.agent("config/agents/a.md", "scout", "background: true\n");
        assert!(f.background("scout"));
        f.agent("home/repo/.claude/agents/a.md", "scout", "");
        assert!(!f.background("scout"), "project overrides user");
        f.agent(
            "home/repo/app/.claude/agents/a.md",
            "scout",
            "background: true\n",
        );
        assert!(f.background("scout"), "the deeper project dir wins");
        f.agent("managed/.claude/agents/a.md", "scout", "");
        assert!(
            !f.background("scout"),
            "managed policy overrides everything"
        );
    }

    #[test]
    fn the_project_walk_stops_at_the_git_root_and_below_home() {
        let f = Fixture::new();
        fs::create_dir_all(f.root.join("home/repo/app/.git")).unwrap();
        f.agent(
            "home/repo/.claude/agents/a.md",
            "above-git-root",
            "background: true\n",
        );
        assert_eq!(f.resolve("above-git-root"), None);

        let f = Fixture::new();
        fs::remove_dir_all(f.root.join("home/repo/.git")).unwrap();
        f.agent(
            "home/repo/.claude/agents/a.md",
            "in-repo",
            "background: true\n",
        );
        f.agent("home/.claude/agents/a.md", "at-home", "background: true\n");
        assert!(f.background("in-repo"), "no git root: walk up to home");
        assert_eq!(f.resolve("at-home"), None, "home itself is never a project");
    }

    #[test]
    fn a_linked_worktree_without_agents_reads_its_main_checkout() {
        let f = Fixture::new();
        let main = f.root.join("home/repo");
        let worktree = f.root.join("home/repo/.wt/thread");
        let git_dir = main.join(".git/worktrees/thread");
        fs::create_dir_all(&git_dir).unwrap();
        fs::write(git_dir.join("commondir"), "../..\n").unwrap();
        fs::create_dir_all(&worktree).unwrap();
        fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", git_dir.display()),
        )
        .unwrap();
        f.agent(
            "home/repo/.claude/agents/a.md",
            "scout",
            "background: true\n",
        );
        let sources = AgentSources {
            cwd: worktree.clone(),
            ..f.sources
        };
        assert!(resolve("scout", &sources).is_some_and(|a| a.background));
    }

    #[test]
    fn an_unknown_type_resolves_to_nothing() {
        let f = Fixture::new();
        assert_eq!(f.resolve("nobody"), None);
    }

    #[test]
    fn a_built_in_never_runs_in_the_background() {
        let f = Fixture::new();
        for built_in in BUILT_IN_AGENT_TYPES {
            assert_eq!(
                f.resolve(built_in),
                Some(AgentDefinition::built_in(built_in))
            );
        }
    }

    /// An exact match on the built-in wins over a loose one on a custom agent.
    #[test]
    fn a_built_in_name_does_not_loosely_match_a_custom_agent() {
        let f = Fixture::new();
        f.agent("config/agents/a.md", "explore", "background: true\n");
        assert!(!f.background("Explore"));
        assert!(f.background("explore"));
    }

    #[test]
    fn a_custom_agent_replaces_the_built_in_of_the_same_name() {
        let f = Fixture::new();
        f.agent(
            "config/agents/a.md",
            "general-purpose",
            "background: true\n",
        );
        assert!(f.background(DEFAULT_AGENT_TYPE));
    }

    #[test]
    fn a_loose_name_matches_a_single_agent() {
        let f = Fixture::new();
        f.agent("config/agents/a.md", "code-reviewer", "background: true\n");
        assert!(f.background("Code_Reviewer"));
        assert!(f.background("code reviewer"));
        assert!(f.background("code\u{2010}reviewer"), "a Unicode hyphen");
        assert!(
            f.background("\u{FF23}ode-reviewer"),
            "a fullwidth letter, by NFKC"
        );
    }

    /// Claude Code refuses an ambiguous loose match, so nothing launches.
    #[test]
    fn an_ambiguous_loose_name_resolves_to_nothing() {
        let f = Fixture::new();
        f.agent("config/agents/a.md", "code-reviewer", "background: true\n");
        f.agent("config/agents/b.md", "code_reviewer", "background: true\n");
        assert_eq!(f.resolve("CodeReviewer"), None);
    }

    /// YAML reads a plain value continued on the next, indented line.
    #[test]
    fn a_description_on_the_next_line_counts() {
        let f = Fixture::new();
        f.write(
            "config/agents/a.md",
            "---\nname: scout\ndescription:\n  Reviews code.\nbackground: true\n---\n",
        );
        assert!(f.background("scout"));
        f.write(
            "config/agents/a.md",
            "---\nname: scout\ndescription:\nbackground: true\n---\n",
        );
        assert!(
            !f.background("scout"),
            "an empty description is still empty"
        );
    }

    #[test]
    fn background_reads_like_yaml() {
        let f = Fixture::new();
        for (value, expected) in [
            ("true", true),
            ("True", true),
            ("TRUE", true),
            ("\"true\"", true),
            ("'true'", true),
            ("true # comment", true),
            ("false", false),
            ("\"True\"", false),
            ("yes", false),
            ("", false),
        ] {
            f.agent(
                "config/agents/a.md",
                "scout",
                &format!("background: {value}\n"),
            );
            assert_eq!(f.background("scout"), expected, "background: {value}");
        }
    }

    #[test]
    fn a_nested_background_key_is_not_the_agent_flag() {
        let f = Fixture::new();
        f.agent(
            "config/agents/a.md",
            "scout",
            "hooks:\n  background: true\n",
        );
        assert!(!f.background("scout"));
    }

    #[test]
    fn isolation_remote_is_read_from_a_directory_agent() {
        let f = Fixture::new();
        f.agent("config/agents/a.md", "far", "isolation: remote\n");
        f.agent("config/agents/b.md", "near", "isolation: worktree\n");
        assert!(f.resolve("far").is_some_and(|a| a.remote_isolation));
        assert!(f.resolve("near").is_some_and(|a| !a.remote_isolation));
    }

    #[test]
    fn a_file_claude_code_would_not_load_is_skipped() {
        let f = Fixture::new();
        f.write(
            "config/agents/no-name.md",
            "---\ndescription: x\nbackground: true\n---\n",
        );
        f.write(
            "config/agents/no-desc.md",
            "---\nname: no-desc\nbackground: true\n---\n",
        );
        f.write(
            "config/agents/colon.md",
            "---\nname: a:b\ndescription: x\nbackground: true\n---\n",
        );
        f.write(
            "config/agents/unclosed.md",
            "---\nname: unclosed\ndescription: x\nbackground: true\n",
        );
        f.write(
            "config/agents/no-fence.md",
            "name: no-fence\ndescription: x\nbackground: true\n",
        );
        f.write(
            "config/agents/notes.txt",
            "---\nname: notes\ndescription: x\n---\n",
        );
        for name in ["no-desc", "a:b", "unclosed", "no-fence", "notes"] {
            assert!(!f.background(name), "{name}");
        }
    }

    #[test]
    fn a_plugin_agent_is_namespaced_by_the_manifest_name_and_subdirectories() {
        let f = Fixture::new();
        let install = f.plugin("toolkit@market", r#"{"name": "kit"}"#);
        f.write(
            &format!(
                "{}/agents/scout.md",
                install.strip_prefix(&f.root).unwrap().display()
            ),
            "---\nbackground: true\n---\n",
        );
        f.write(
            &format!(
                "{}/agents/team/lead.md",
                install.strip_prefix(&f.root).unwrap().display()
            ),
            "---\nname: chief\nbackground: true\nisolation: remote\n---\n",
        );
        assert!(f.background("kit:scout"), "the file stem names it");
        let chief = f.resolve("kit:team:chief").expect("nested agent");
        assert!(chief.background);
        assert!(
            !chief.remote_isolation,
            "plugins honour only worktree isolation"
        );
        assert_eq!(f.resolve("toolkit:scout"), None);
    }

    #[test]
    fn a_plugin_manifest_can_name_extra_agent_paths() {
        let f = Fixture::new();
        let install = f.plugin(
            "toolkit@market",
            r#"{"name": "kit", "agents": ["./more/one.md", "./extra"]}"#,
        );
        let base = install.strip_prefix(&f.root).unwrap().display().to_string();
        f.write(
            &format!("{base}/more/one.md"),
            "---\nbackground: true\n---\n",
        );
        f.write(
            &format!("{base}/extra/two.md"),
            "---\nbackground: true\n---\n",
        );
        assert!(f.background("kit:one"));
        assert!(f.background("kit:two"));
    }

    #[test]
    fn a_plugin_without_a_manifest_name_uses_its_id() {
        let f = Fixture::new();
        let install = f.plugin("toolkit@market", "{}");
        let base = install.strip_prefix(&f.root).unwrap().display().to_string();
        f.write(
            &format!("{base}/agents/scout.md"),
            "---\nbackground: true\n---\n",
        );
        assert!(f.background("toolkit:scout"));
    }

    #[test]
    fn a_disabled_plugin_contributes_no_agents() {
        let f = Fixture::new();
        let install = f.plugin("toolkit@market", r#"{"name": "kit"}"#);
        let base = install.strip_prefix(&f.root).unwrap().display().to_string();
        f.write(
            &format!("{base}/agents/scout.md"),
            "---\nbackground: true\n---\n",
        );
        assert!(f.background("kit:scout"));
        let disable = r#"{"enabledPlugins": {"toolkit@market": false}}"#;
        f.write("home/repo/app/.claude/settings.local.json", disable);
        assert!(
            f.background("kit:scout"),
            "settings below the session root do not apply"
        );
        f.write("home/repo/.claude/settings.local.json", disable);
        assert_eq!(
            f.resolve("kit:scout"),
            None,
            "a later settings file disables it"
        );
    }

    /// The session `cwd` is a subdirectory of the project, so the install's
    /// project is matched at the git root.
    #[test]
    fn a_project_install_counts_only_in_its_project() {
        let f = Fixture::new();
        let install = f.plugin("toolkit@market", r#"{"name": "kit"}"#);
        let base = install.strip_prefix(&f.root).unwrap().display().to_string();
        f.write(
            &format!("{base}/agents/scout.md"),
            "---\nbackground: true\n---\n",
        );
        let installed = |project: &Path| {
            serde_json::json!({"plugins": {"toolkit@market": [
                {"scope": "project", "projectPath": project, "installPath": install},
            ]}})
            .to_string()
        };
        f.write(
            "config/plugins/installed_plugins.json",
            &installed(Path::new("/elsewhere")),
        );
        assert_eq!(f.resolve("kit:scout"), None);
        f.write(
            "config/plugins/installed_plugins.json",
            &installed(&f.root.join("home/repo")),
        );
        assert!(f.background("kit:scout"));
    }

    #[test]
    fn unreadable_sources_resolve_to_nothing() {
        let f = Fixture::new();
        f.write("config/settings.json", "{not json");
        f.write("config/plugins/installed_plugins.json", "[]");
        assert_eq!(f.resolve("kit:scout"), None);
        let sources = AgentSources {
            cwd: f.root.join("missing"),
            home: None,
            config_dir: None,
            managed_dir: f.root.join("missing"),
            additional_dirs: vec![f.root.join("missing")],
        };
        assert_eq!(resolve("scout", &sources), None);
    }
}

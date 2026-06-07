use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use goat_protocol::Effort;

pub enum ToolSelection {
    All,
    Only(Vec<String>),
}

impl ToolSelection {
    pub fn allows(&self, name: &str) -> bool {
        match self {
            ToolSelection::All => true,
            ToolSelection::Only(list) => list.iter().any(|tool| tool == name),
        }
    }
}

pub struct AgentSpec {
    pub name: String,
    pub description: String,
    pub tools: ToolSelection,
    pub model: Option<String>,
    pub effort: Option<Effort>,
    pub prompt: String,
}

pub struct AgentRegistry {
    agents: BTreeMap<String, AgentSpec>,
}

impl AgentRegistry {
    pub fn load(cwd: &Path) -> Self {
        let mut agents: BTreeMap<String, AgentSpec> = BTreeMap::new();
        for spec in builtin_agents() {
            agents.insert(spec.name.clone(), spec);
        }
        let mut dirs: Vec<PathBuf> = Vec::new();
        if let Some(global) = goat_config::agents_dir() {
            dirs.push(global);
        }
        dirs.push(cwd.join(goat_config::PROJECT_AGENTS_SUBDIR));
        for dir in &dirs {
            load_dir(dir, &mut agents);
        }
        Self { agents }
    }

    pub fn get(&self, name: &str) -> Option<&AgentSpec> {
        self.agents.get(name)
    }

    pub fn iter(&self) -> impl Iterator<Item = &AgentSpec> {
        self.agents.values()
    }

    pub fn names(&self) -> Vec<String> {
        self.agents.keys().cloned().collect()
    }

    pub fn is_empty(&self) -> bool {
        self.agents.is_empty()
    }
}

const EXPLORE_PROMPT: &str = "You are an exploration agent investigating a codebase to answer one specific question. You have read-only tools (Read, Grep, Glob) and cannot modify anything. Search broadly, read the relevant files, and trace how the pieces connect. Return a concise, well-structured report: the concrete answer, the key files and line references that support it, and any important caveats. Do not speculate beyond what you verified.";

const GENERAL_PROMPT: &str = "You are a general-purpose agent handling a delegated task end to end. Use the available tools to complete the task, verify the result, and return a concise summary of what you did and the outcome.";

fn builtin_agents() -> Vec<AgentSpec> {
    vec![
        AgentSpec {
            name: "explore".to_owned(),
            description: "Read-only codebase exploration: searches and reads files to investigate a question and report findings without making changes. Launch several in parallel for independent areas.".to_owned(),
            tools: ToolSelection::Only(vec![
                "Read".to_owned(),
                "Grep".to_owned(),
                "Glob".to_owned(),
            ]),
            model: None,
            effort: None,
            prompt: EXPLORE_PROMPT.to_owned(),
        },
        AgentSpec {
            name: "general".to_owned(),
            description: "General-purpose agent with full tools for a multi-step delegated task.".to_owned(),
            tools: ToolSelection::All,
            model: None,
            effort: None,
            prompt: GENERAL_PROMPT.to_owned(),
        },
    ]
}

fn load_dir(dir: &Path, out: &mut BTreeMap<String, AgentSpec>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            load_dir(&path, out);
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let stem = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        match parse(&content, &stem) {
            Ok(spec) => {
                out.insert(spec.name.clone(), spec);
            }
            Err(reason) => {
                tracing::warn!(path = %path.display(), reason, "skipping agent");
            }
        }
    }
}

fn parse(content: &str, stem: &str) -> Result<AgentSpec, &'static str> {
    let content = content.trim_start_matches('\u{feff}');
    let mut lines = content.lines();
    if lines.next().map(str::trim_end) != Some("---") {
        return Err("missing frontmatter");
    }
    let mut name: Option<String> = None;
    let mut description: Option<String> = None;
    let mut tools_raw: Option<String> = None;
    let mut model: Option<String> = None;
    let mut effort: Option<Effort> = None;
    let mut closed = false;
    let mut body_lines: Vec<&str> = Vec::new();
    for line in lines {
        if closed {
            body_lines.push(line);
            continue;
        }
        if line.trim_end() == "---" {
            closed = true;
            continue;
        }
        if let Some((key, value)) = line.split_once(':') {
            let value = unquote(value.trim()).to_owned();
            match key.trim() {
                "name" => name = Some(value),
                "description" => description = Some(value),
                "tools" => tools_raw = Some(value),
                "model" => model = Some(value),
                "effort" => effort = Effort::parse(&value),
                _ => {}
            }
        }
    }
    if !closed {
        return Err("unterminated frontmatter");
    }
    let name = name
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| stem.to_owned());
    let description = description
        .filter(|desc| !desc.is_empty())
        .ok_or("missing description")?;
    let tools = match tools_raw {
        Some(raw) if !raw.trim().is_empty() => ToolSelection::Only(
            raw.split(',')
                .map(|tool| tool.trim().to_owned())
                .filter(|tool| !tool.is_empty())
                .collect(),
        ),
        _ => ToolSelection::All,
    };
    let model = model.filter(|model| !model.is_empty());
    let prompt = body_lines.join("\n").trim().to_owned();
    Ok(AgentSpec {
        name,
        description,
        tools,
        model,
        effort,
        prompt,
    })
}

fn unquote(value: &str) -> &str {
    let bytes = value.as_bytes();
    let len = bytes.len();
    if len >= 2
        && ((bytes[0] == b'"' && bytes[len - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[len - 1] == b'\''))
    {
        &value[1..len - 1]
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::{AgentRegistry, ToolSelection, parse};

    #[test]
    fn builtins_present() {
        let registry = AgentRegistry::load(std::path::Path::new("/nonexistent-agents-dir"));
        let explore = registry.get("explore").expect("explore builtin");
        assert!(explore.tools.allows("Read"));
        assert!(explore.tools.allows("Grep"));
        assert!(!explore.tools.allows("Write"));
        let general = registry.get("general").expect("general builtin");
        assert!(general.tools.allows("Write"));
    }

    #[test]
    fn parses_tools_list() {
        let spec = parse(
            "---\nname: rev\ndescription: review code\ntools: Read, Grep\nmodel: haiku\n---\nReview carefully.\n",
            "file-stem",
        )
        .unwrap();
        assert_eq!(spec.name, "rev");
        assert_eq!(spec.description, "review code");
        assert_eq!(spec.model.as_deref(), Some("haiku"));
        assert!(spec.tools.allows("Read"));
        assert!(spec.tools.allows("Grep"));
        assert!(!spec.tools.allows("Bash"));
        assert_eq!(spec.prompt, "Review carefully.");
    }

    #[test]
    fn omitted_tools_means_all() {
        let spec = parse("---\ndescription: d\n---\nbody", "doer").unwrap();
        assert_eq!(spec.name, "doer");
        assert!(matches!(spec.tools, ToolSelection::All));
        assert!(spec.tools.allows("Write"));
    }

    #[test]
    fn missing_description_errors() {
        assert!(parse("---\nname: x\n---\nbody", "x").is_err());
    }

    #[test]
    fn file_agent_overrides_builtin() {
        let dir = tempfile::tempdir().unwrap();
        let agents = dir.path().join(".goat/agents");
        std::fs::create_dir_all(&agents).unwrap();
        std::fs::write(
            agents.join("explore.md"),
            "---\nname: explore\ndescription: custom\ntools: Read\n---\nCustom explore.\n",
        )
        .unwrap();
        let registry = AgentRegistry::load(dir.path());
        let explore = registry.get("explore").unwrap();
        assert_eq!(explore.description, "custom");
        assert!(!explore.tools.allows("Grep"));
    }
}

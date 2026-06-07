use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

pub struct Skill {
    pub name: String,
    pub description: String,
    pub dir: PathBuf,
    pub body: String,
}

pub struct SkillSet {
    skills: BTreeMap<String, Skill>,
}

impl SkillSet {
    pub fn get(&self, name: &str) -> Option<&Skill> {
        self.skills.get(name)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Skill> {
        self.skills.values()
    }

    pub fn len(&self) -> usize {
        self.skills.len()
    }

    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }
}

pub fn load(cwd: &Path) -> SkillSet {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(global) = goat_config::skills_dir() {
        dirs.push(global);
    }
    dirs.push(cwd.join(goat_config::PROJECT_SKILLS_SUBDIR));
    load_from_dirs(&dirs)
}

pub fn load_from_dirs(dirs: &[PathBuf]) -> SkillSet {
    let mut skills: BTreeMap<String, Skill> = BTreeMap::new();
    for dir in dirs {
        load_dir(dir, &mut skills);
    }
    SkillSet { skills }
}

fn load_dir(dir: &Path, out: &mut BTreeMap<String, Skill>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let manifest = path.join("SKILL.md");
        let Ok(content) = std::fs::read_to_string(&manifest) else {
            continue;
        };
        let dir_name = entry.file_name().to_string_lossy().into_owned();
        match parse(&content, &dir_name) {
            Ok(parsed) => {
                out.insert(
                    parsed.name.clone(),
                    Skill {
                        name: parsed.name,
                        description: parsed.description,
                        dir: path,
                        body: parsed.body,
                    },
                );
            }
            Err(reason) => {
                tracing::warn!(path = %manifest.display(), reason, "skipping skill");
            }
        }
    }
}

struct Parsed {
    name: String,
    description: String,
    body: String,
}

fn parse(content: &str, dir_name: &str) -> Result<Parsed, &'static str> {
    let content = content.trim_start_matches('\u{feff}');
    let mut lines = content.lines();
    if lines.next().map(str::trim_end) != Some("---") {
        return Err("missing frontmatter");
    }
    let mut name: Option<String> = None;
    let mut description: Option<String> = None;
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
                _ => {}
            }
        }
    }
    if !closed {
        return Err("unterminated frontmatter");
    }
    let name = name
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| dir_name.to_owned());
    let description = description
        .filter(|d| !d.is_empty())
        .ok_or("missing description")?;
    Ok(Parsed {
        name,
        description,
        body: body_lines.join("\n").trim().to_owned(),
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
    use super::{load_from_dirs, parse};

    fn write_skill(dir: &std::path::Path, name: &str, contents: &str) {
        let skill_dir = dir.join(name);
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), contents).unwrap();
    }

    #[test]
    fn parses_frontmatter_and_body() {
        let parsed = parse(
            "---\nname: greet\ndescription: \"Say hello\"\n---\n\nDo the greeting.\n",
            "greet-dir",
        )
        .unwrap();
        assert_eq!(parsed.name, "greet");
        assert_eq!(parsed.description, "Say hello");
        assert_eq!(parsed.body, "Do the greeting.");
    }

    #[test]
    fn name_falls_back_to_dir() {
        let parsed = parse("---\ndescription: x\n---\nbody", "from-dir").unwrap();
        assert_eq!(parsed.name, "from-dir");
    }

    #[test]
    fn missing_description_is_error() {
        assert!(parse("---\nname: x\n---\nbody", "d").is_err());
    }

    #[test]
    fn missing_frontmatter_is_error() {
        assert!(parse("no frontmatter here", "d").is_err());
    }

    #[test]
    fn scans_directory() {
        let dir = tempfile::tempdir().unwrap();
        write_skill(dir.path(), "alpha", "---\ndescription: A\n---\nalpha body");
        write_skill(dir.path(), "beta", "---\ndescription: B\n---\nbeta body");
        let set = load_from_dirs(&[dir.path().to_path_buf()]);
        assert_eq!(set.len(), 2);
        assert_eq!(set.get("alpha").unwrap().body, "alpha body");
        assert_eq!(set.get("beta").unwrap().description, "B");
    }

    #[test]
    fn project_overrides_global() {
        let global = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        write_skill(
            global.path(),
            "shared",
            "---\ndescription: global\n---\nglobal body",
        );
        write_skill(
            project.path(),
            "shared",
            "---\ndescription: project\n---\nproject body",
        );
        let set = load_from_dirs(&[global.path().to_path_buf(), project.path().to_path_buf()]);
        assert_eq!(set.len(), 1);
        assert_eq!(set.get("shared").unwrap().body, "project body");
        assert_eq!(set.get("shared").unwrap().description, "project");
    }
}

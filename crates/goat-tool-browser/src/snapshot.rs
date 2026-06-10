use std::fmt::Write as _;

use serde::Deserialize;

pub const SNAPSHOT_JS: &str = r"(() => {
  const out = [];
  const MAX = 800;
  let refCount = 0;
  let truncated = false;

  document.querySelectorAll('[data-goat-ref]').forEach(e => e.removeAttribute('data-goat-ref'));

  const visible = (el) => {
    const s = getComputedStyle(el);
    if (s.display === 'none' || s.visibility === 'hidden' || s.opacity === '0') return false;
    if (el.getAttribute('aria-hidden') === 'true') return false;
    if (el.getClientRects().length === 0) return false;
    return true;
  };

  const roleOf = (el) => {
    const explicit = el.getAttribute('role');
    if (explicit) return explicit;
    const tag = el.tagName.toLowerCase();
    if (tag === 'a') return 'link';
    if (tag === 'button') return 'button';
    if (tag === 'select') return 'combobox';
    if (tag === 'textarea') return 'textbox';
    if (tag === 'summary') return 'button';
    if (tag === 'input') {
      const t = (el.getAttribute('type') || 'text').toLowerCase();
      if (t === 'submit' || t === 'button' || t === 'reset') return 'button';
      if (t === 'checkbox') return 'checkbox';
      if (t === 'radio') return 'radio';
      return 'textbox';
    }
    return 'generic';
  };

  const interactiveRoles = ['button','link','checkbox','radio','switch','tab','menuitem','combobox','option','searchbox','textbox','slider'];
  const interactive = (el) => {
    const tag = el.tagName.toLowerCase();
    if (tag === 'a') return el.hasAttribute('href');
    if (['button','input','select','textarea','summary'].includes(tag)) return true;
    if (el.hasAttribute('onclick')) return true;
    if (el.isContentEditable) return true;
    const role = el.getAttribute('role');
    if (role && interactiveRoles.includes(role)) return true;
    const ti = el.getAttribute('tabindex');
    if (ti !== null && parseInt(ti, 10) >= 0) return true;
    return false;
  };

  const clean = (s) => (s || '').trim().replace(/\s+/g, ' ');

  const nameOf = (el) => {
    const aria = el.getAttribute('aria-label');
    if (aria) return clean(aria);
    const by = el.getAttribute('aria-labelledby');
    if (by) {
      const target = document.getElementById(by);
      if (target) return clean(target.innerText).slice(0, 80);
    }
    const ph = el.getAttribute('placeholder');
    if (ph) return clean(ph);
    const alt = el.getAttribute('alt');
    if (alt) return clean(alt);
    const title = el.getAttribute('title');
    if (title) return clean(title);
    return clean(el.innerText || el.value).slice(0, 80);
  };

  const statesOf = (el) => {
    const s = [];
    if (el.disabled) s.push('disabled');
    if (el.checked || el.getAttribute('aria-checked') === 'true') s.push('checked');
    if (el.getAttribute('aria-expanded') === 'true') s.push('expanded');
    if (el.required) s.push('required');
    return s;
  };

  const valueOf = (el) => {
    const tag = el.tagName.toLowerCase();
    if (tag === 'input' || tag === 'textarea') {
      const t = (el.getAttribute('type') || 'text').toLowerCase();
      if (t === 'password') return el.value ? '***' : '';
      return (el.value || '').slice(0, 40);
    }
    if (tag === 'select') {
      const o = el.options[el.selectedIndex];
      return o ? clean(o.text).slice(0, 40) : '';
    }
    return '';
  };

  const ownText = (el) => {
    let s = '';
    for (const n of el.childNodes) {
      if (n.nodeType === 3) s += n.textContent;
    }
    return clean(s);
  };

  const textTags = ['P','LI','TD','TH','DT','DD','LABEL','LEGEND','FIGCAPTION','BLOCKQUOTE'];

  const walk = (el, depth) => {
    if (out.length >= MAX) { truncated = true; return; }
    if (el.nodeType !== 1 || !visible(el)) return;
    const tag = el.tagName;
    let next = depth;

    if (interactive(el)) {
      refCount += 1;
      const ref = 'e' + refCount;
      el.setAttribute('data-goat-ref', ref);
      const node = { depth: depth, role: roleOf(el), name: nameOf(el), ref: ref };
      const st = statesOf(el);
      if (st.length) node.states = st;
      const v = valueOf(el);
      if (v) node.value = v;
      out.push(node);
      next = depth + 1;
    } else if (/^H[1-6]$/.test(tag)) {
      out.push({ depth: depth, role: 'heading', name: clean(el.innerText).slice(0, 80), level: parseInt(tag[1], 10) });
      next = depth + 1;
    } else if (tag === 'IFRAME') {
      out.push({ depth: depth, text: '[iframe: ' + (el.getAttribute('src') || '') + ' - content not included]' });
      return;
    } else if (textTags.includes(tag)) {
      const t = ownText(el);
      if (t) { out.push({ depth: depth, text: t.slice(0, 120) }); next = depth + 1; }
    }

    for (const child of el.children) walk(child, next);
  };

  if (document.body) walk(document.body, 0);
  return { title: document.title || '', nodes: out, truncated: truncated };
})()";

#[derive(Debug, Deserialize)]
pub struct RawSnapshot {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub nodes: Vec<RawNode>,
    #[serde(default)]
    pub truncated: bool,
}

#[derive(Debug, Deserialize)]
pub struct RawNode {
    pub depth: usize,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub name: String,
    #[serde(default, rename = "ref")]
    pub reference: Option<String>,
    #[serde(default)]
    pub states: Vec<String>,
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub level: Option<u8>,
    #[serde(default)]
    pub text: Option<String>,
}

pub fn format_snapshot(url: &str, raw: &RawSnapshot, max_bytes: usize) -> String {
    let mut out = format!("Page: {}\nURL: {url}\n\n", raw.title);
    for node in &raw.nodes {
        let indent = "  ".repeat(node.depth);
        if node.role.is_empty() {
            if let Some(text) = &node.text {
                let _ = writeln!(out, "{indent}- text: {text}");
            }
            continue;
        }
        if node.role == "heading" {
            let level = node.level.unwrap_or(0);
            let _ = writeln!(out, "{indent}- heading \"{}\" [level={level}]", node.name);
            continue;
        }
        let reference = node.reference.as_deref().unwrap_or("");
        let _ = write!(
            out,
            "{indent}- {} \"{}\" [ref={reference}]",
            node.role, node.name
        );
        if let Some(value) = &node.value {
            let _ = write!(out, " value=\"{value}\"");
        }
        if !node.states.is_empty() {
            let _ = write!(out, " ({})", node.states.join(", "));
        }
        out.push('\n');
    }
    if raw.truncated {
        out.push_str("[page tree truncated at 800 nodes]\n");
    }
    truncate_bytes(out, max_bytes)
}

fn truncate_bytes(mut s: String, max_bytes: usize) -> String {
    if s.len() > max_bytes {
        let boundary = s.floor_char_boundary(max_bytes);
        s.truncate(boundary);
        s.push_str("\n[snapshot truncated]\n");
    }
    s
}

pub fn is_valid_ref(s: &str) -> bool {
    s.strip_prefix('e')
        .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::{RawNode, RawSnapshot, format_snapshot, is_valid_ref};

    fn node(depth: usize, role: &str, name: &str, reference: Option<&str>) -> RawNode {
        RawNode {
            depth,
            role: role.to_owned(),
            name: name.to_owned(),
            reference: reference.map(ToOwned::to_owned),
            states: Vec::new(),
            value: None,
            level: None,
            text: None,
        }
    }

    #[test]
    fn renders_header_and_elements() {
        let raw = RawSnapshot {
            title: "Dashboard".to_owned(),
            nodes: vec![
                node(0, "button", "Submit", Some("e1")),
                node(1, "link", "Home", Some("e2")),
            ],
            truncated: false,
        };
        let out = format_snapshot("https://x.com", &raw, 64 * 1024);
        assert!(out.starts_with("Page: Dashboard\nURL: https://x.com\n"));
        assert!(out.contains("- button \"Submit\" [ref=e1]"));
        assert!(out.contains("  - link \"Home\" [ref=e2]"));
    }

    #[test]
    fn renders_value_and_states() {
        let mut email = node(0, "textbox", "Email", Some("e1"));
        email.value = Some("jo@x.com".to_owned());
        let mut check = node(0, "checkbox", "Remember", Some("e2"));
        check.states = vec!["checked".to_owned(), "disabled".to_owned()];
        let raw = RawSnapshot {
            title: String::new(),
            nodes: vec![email, check],
            truncated: false,
        };
        let out = format_snapshot("about:blank", &raw, 64 * 1024);
        assert!(out.contains("- textbox \"Email\" [ref=e1] value=\"jo@x.com\""));
        assert!(out.contains("- checkbox \"Remember\" [ref=e2] (checked, disabled)"));
    }

    #[test]
    fn renders_heading_and_text() {
        let heading = RawNode {
            level: Some(1),
            ..node(0, "heading", "Welcome", None)
        };
        let text = RawNode {
            text: Some("Body copy.".to_owned()),
            ..node(1, "", "", None)
        };
        let raw = RawSnapshot {
            title: "T".to_owned(),
            nodes: vec![heading, text],
            truncated: false,
        };
        let out = format_snapshot("about:blank", &raw, 64 * 1024);
        assert!(out.contains("- heading \"Welcome\" [level=1]"));
        assert!(out.contains("  - text: Body copy."));
    }

    #[test]
    fn marks_truncation() {
        let raw = RawSnapshot {
            title: "T".to_owned(),
            nodes: vec![node(0, "button", "Go", Some("e1"))],
            truncated: true,
        };
        let out = format_snapshot("about:blank", &raw, 64 * 1024);
        assert!(out.contains("[page tree truncated at 800 nodes]"));
    }

    #[test]
    fn caps_output_on_char_boundary() {
        let nodes = (1..=200)
            .map(|i| node(0, "button", "행동하기버튼", Some("e1")).with_name_index(i))
            .collect();
        let raw = RawSnapshot {
            title: "T".to_owned(),
            nodes,
            truncated: false,
        };
        let out = format_snapshot("about:blank", &raw, 256);
        assert!(out.len() <= 256 + "\n[snapshot truncated]\n".len());
        assert!(out.contains("[snapshot truncated]"));
        assert!(std::str::from_utf8(out.as_bytes()).is_ok());
    }

    #[test]
    fn validates_refs() {
        for good in ["e1", "e12", "e007"] {
            assert!(is_valid_ref(good), "{good} should be valid");
        }
        for bad in ["e", "1", "e1']", "", "x1", "e1 "] {
            assert!(!is_valid_ref(bad), "{bad} should be invalid");
        }
    }

    impl RawNode {
        fn with_name_index(mut self, i: usize) -> Self {
            self.name = format!("{}{i}", self.name);
            self
        }
    }
}

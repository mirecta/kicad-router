//! Which net class each net belongs to — read from the project's
//! `net_settings` (KiCad 8+ keeps it there, not in the board file).
//! Semantics verified against real `kicad-cli` (see `docs/DECISIONS.md`'s
//! "Net classes from the project" entry):
//! - a class applies to a net through an explicit `netclass_assignments`
//!   entry (a class name or a list of them) or any matching
//!   `netclass_patterns` entry;
//! - a pattern matches the **whole** net name, case-sensitively, either as
//!   a wildcard (`*`, `?`) or as a regular expression;
//! - of all classes that apply, the one with the lowest `priority` number
//!   wins, ties going to the alphabetically first name — an explicit
//!   assignment has no extra precedence over a pattern;
//! - no class applies → `Default`.

use std::collections::HashMap;

use regex::Regex;

struct Pattern {
    text: String,
    regex: Option<Regex>,
    class: String,
}

impl Pattern {
    fn matches(&self, name: &str) -> bool {
        wildcard_match(&self.text, name) || self.regex.as_ref().is_some_and(|r| r.is_match(name))
    }
}

/// Full-string wildcard match: `*` any run of characters, `?` any one.
fn wildcard_match(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let s: Vec<char> = name.chars().collect();
    let (mut pi, mut si) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while si < s.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == s[si]) {
            pi += 1;
            si += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some((pi, si));
            pi += 1;
        } else if let Some((sp, ss)) = star {
            pi = sp + 1;
            si = ss + 1;
            star = Some((sp, ss + 1));
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

/// The project's net-class rules.
pub(crate) struct NetClassRules {
    priorities: HashMap<String, i64>,
    assignments: HashMap<String, Vec<String>>,
    patterns: Vec<Pattern>,
}

impl NetClassRules {
    pub(crate) fn from_project(json: &serde_json::Value) -> Self {
        let settings = json.pointer("/net_settings");
        let priorities = settings
            .and_then(|s| s.get("classes"))
            .and_then(serde_json::Value::as_array)
            .map(|classes| {
                classes
                    .iter()
                    .filter_map(|c| {
                        let name = c.get("name")?.as_str()?.to_string();
                        let priority = c.get("priority").and_then(serde_json::Value::as_i64);
                        Some((name, priority.unwrap_or(i64::MAX)))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let assignments = settings
            .and_then(|s| s.get("netclass_assignments"))
            .and_then(serde_json::Value::as_object)
            .map(|map| {
                map.iter()
                    .map(|(net, v)| {
                        let classes = match v {
                            serde_json::Value::String(c) => vec![c.clone()],
                            serde_json::Value::Array(cs) => cs
                                .iter()
                                .filter_map(|c| c.as_str().map(str::to_string))
                                .collect(),
                            _ => Vec::new(),
                        };
                        (net.clone(), classes)
                    })
                    .collect()
            })
            .unwrap_or_default();
        let patterns = settings
            .and_then(|s| s.get("netclass_patterns"))
            .and_then(serde_json::Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(|p| {
                        let text = p.get("pattern")?.as_str()?.to_string();
                        let class = p.get("netclass")?.as_str()?.to_string();
                        let regex = Regex::new(&format!("^(?:{text})$")).ok();
                        Some(Pattern { text, regex, class })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self {
            priorities,
            assignments,
            patterns,
        }
    }

    /// The class `net_name` belongs to, or `None` for `Default`. Only
    /// classes the project actually defines are considered.
    pub(crate) fn class_for(&self, net_name: &str) -> Option<&str> {
        let assigned = self
            .assignments
            .get(net_name)
            .into_iter()
            .flatten()
            .map(String::as_str);
        let matched = self
            .patterns
            .iter()
            .filter(|p| p.matches(net_name))
            .map(|p| p.class.as_str());
        assigned
            .chain(matched)
            .filter_map(|c| self.priorities.get(c).map(|&prio| (prio, c)))
            .min()
            .map(|(_, c)| c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(json: &str) -> NetClassRules {
        NetClassRules::from_project(&serde_json::from_str(json).unwrap())
    }

    const CLASSES: &str = r#""classes": [
        {"name": "Default", "priority": 2147483647},
        {"name": "Power", "priority": 5},
        {"name": "HV", "priority": 5}
    ]"#;

    #[test]
    fn wildcards_and_regexes_match_the_whole_name_case_sensitively() {
        let r = rules(&format!(
            r#"{{"net_settings": {{{CLASSES}, "netclass_patterns": [
                {{"netclass": "Power", "pattern": "+*V"}},
                {{"netclass": "Power", "pattern": "GN?"}},
                {{"netclass": "Power", "pattern": "^V(IN|BUS)$"}}
            ]}}}}"#
        ));
        assert_eq!(r.class_for("+5V"), Some("Power"));
        assert_eq!(r.class_for("GND"), Some("Power"));
        assert_eq!(r.class_for("VBUS"), Some("Power"));
        assert_eq!(r.class_for("AGND"), None);
        assert_eq!(r.class_for("gnd"), None);
        assert_eq!(r.class_for("SIG"), None);
    }

    #[test]
    fn lowest_priority_number_wins_then_the_alphabetically_first_name() {
        let r = rules(&format!(
            r#"{{"net_settings": {{{CLASSES},
                "netclass_assignments": {{"GND": "Power"}},
                "netclass_patterns": [{{"netclass": "HV", "pattern": "GND"}}]
            }}}}"#
        ));
        // Equal priority: "HV" sorts first — the assignment doesn't win by itself.
        assert_eq!(r.class_for("GND"), Some("HV"));
        let r = rules(
            r#"{"net_settings": {"classes": [{"name": "Power", "priority": 0}, {"name": "HV", "priority": 1}],
                "netclass_assignments": {"GND": ["Power"]},
                "netclass_patterns": [{"netclass": "HV", "pattern": "GND"}]}}"#,
        );
        assert_eq!(r.class_for("GND"), Some("Power"));
    }

    #[test]
    fn a_class_the_project_doesnt_define_is_ignored() {
        let r =
            rules(r#"{"net_settings": {"classes": [], "netclass_assignments": {"GND": "Ghost"}}}"#);
        assert_eq!(r.class_for("GND"), None);
    }
}

//! Parameter space: parse `.params` files (same format as `param_reader.rb`).
//!
//! Format recap:
//!   alpha {1.01, 1.066, 1.189} [1.189]          # discrete domain, default value
//!   child {a, b} [a] | parent in {val1}          # conditional: only active when parent=val1
//!   {alpha=1.01, rho=0}                          # forbidden combination

use std::collections::HashMap;
use anyhow::{Context, Result};

/// A configuration is a map from parameter name → value string.
pub type Config = HashMap<String, String>;

/// A single parameter: its name, ordered domain, default, and optional condition.
#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub domain: Vec<String>,
    pub default: String,
    pub condition: Option<Condition>,
}

/// `child_param | parent_param in {val1, val2}`
#[derive(Debug, Clone)]
pub struct Condition {
    pub parent: String,
    pub allowed_values: Vec<String>,
}

/// Forbidden combination: a set of `(param, value)` pairs that must not all be active together.
pub type Forbidden = Vec<(String, String)>;

/// Parsed parameter space.
#[derive(Debug, Default)]
pub struct ParamSpace {
    pub params: Vec<Param>,
    pub forbidden: Vec<Forbidden>,
}

impl ParamSpace {
    pub fn from_file(path: &str) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read params file: {path}"))?;

        let mut params: Vec<Param> = Vec::new();
        let mut forbidden: Vec<Forbidden> = Vec::new();

        for (lineno, raw) in content.lines().enumerate() {
            let line = strip_comment(raw).trim().to_string();
            if line.is_empty() {
                continue;
            }

            let has_brace = line.contains('{');
            let has_bracket = line.contains('[');
            let has_pipe = line.contains('|');

            if !has_brace {
                continue;
            }

            if !has_bracket && !has_pipe {
                // Forbidden combo: {p1=v1, p2=v2, ...}
                if let (Some(open), Some(close)) = (line.find('{'), line.rfind('}')) {
                    let combo: Forbidden = line[open + 1..close]
                        .split(',')
                        .filter_map(|part| {
                            let mut it = part.trim().splitn(2, '=');
                            let k = it.next()?.trim().to_string();
                            let v = it.next()?.trim().to_string();
                            if k.is_empty() { return None; }
                            Some((k, v))
                        })
                        .collect();
                    if !combo.is_empty() {
                        forbidden.push(combo);
                    }
                }
                continue;
            }

            if !has_bracket {
                // Might be a standalone condition line: "name | parent in {values}"
                // (condition declared separately from the param definition, as used by some
                // param files like params-eprover.txt)
                if has_pipe {
                    let pipe_idx = line.find('|').unwrap();
                    let name_part = line[..pipe_idx].trim();
                    if !name_part.contains('{') && !name_part.is_empty() {
                        let cond_str = line[pipe_idx + 1..].trim();
                        if let Some(cond) = parse_condition_str(cond_str) {
                            if let Some(p) = params.iter_mut().find(|p| p.name == name_part) {
                                p.condition = Some(cond);
                            }
                        }
                    }
                }
                continue;
            }

            // Param line: name {domain} [default] | optional condition
            let (param_part, cond_str): (&str, Option<&str>) = if has_pipe {
                let pipe_idx = line.find('|').unwrap();
                (line[..pipe_idx].trim(), Some(line[pipe_idx + 1..].trim()))
            } else {
                (line.as_str(), None)
            };

            let open_brace = match param_part.find('{') {
                Some(i) => i,
                None => continue,
            };
            let close_brace = match param_part.find('}') {
                Some(i) => i,
                None => continue,
            };
            let open_bracket = match param_part.find('[') {
                Some(i) => i,
                None => continue,
            };
            let close_bracket = match param_part.find(']') {
                Some(i) => i,
                None => continue,
            };

            let name = param_part[..open_brace].trim().to_string();
            if name.is_empty() {
                continue;
            }

            let domain: Vec<String> = param_part[open_brace + 1..close_brace]
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();

            let default = param_part[open_bracket + 1..close_bracket].trim().to_string();

            if !domain.contains(&default) {
                anyhow::bail!(
                    "line {}: default '{}' not in domain {:?} for param '{}'",
                    lineno + 1,
                    default,
                    domain,
                    name
                );
            }

            let condition = cond_str.map(|cond| {
                parse_condition_str(cond).with_context(|| {
                    format!("line {}: malformed condition '{cond}'", lineno + 1)
                })
            }).transpose()?;

            params.push(Param { name, domain, default, condition });
        }

        Ok(ParamSpace { params, forbidden })
    }

    /// Default configuration: every param at its default value.
    pub fn default_config(&self) -> Config {
        self.params.iter().map(|p| (p.name.clone(), p.default.clone())).collect()
    }

    /// Return only params that are active for `config`.
    ///
    /// A param is active when it has no condition, or when its parent param is
    /// also active and the parent's value is in the allowed set.  Iterates until
    /// stable to handle transitive conditionals.
    pub fn active_params<'a>(&'a self, config: &Config) -> Vec<&'a Param> {
        let n = self.params.len();
        let mut active = vec![true; n];
        loop {
            let mut changed = false;
            for i in 0..n {
                if !active[i] {
                    continue;
                }
                if let Some(cond) = &self.params[i].condition {
                    let parent_active = self.params.iter().enumerate()
                        .any(|(j, p)| active[j] && p.name == cond.parent);
                    let parent_val_ok = config
                        .get(&cond.parent)
                        .map_or(false, |v| cond.allowed_values.contains(v));
                    if !parent_active || !parent_val_ok {
                        active[i] = false;
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }
        self.params.iter().enumerate()
            .filter(|(i, _)| active[*i])
            .map(|(_, p)| p)
            .collect()
    }

    /// Returns true if `config` matches any forbidden combination.
    pub fn is_forbidden(&self, config: &Config) -> bool {
        'outer: for combo in &self.forbidden {
            for (param, val) in combo {
                if config.get(param).map_or(true, |v| v != val) {
                    continue 'outer;
                }
            }
            return true;
        }
        false
    }
}

/// Parse `"parent in {val1, val2, ...}"` into a `Condition`.  Returns `None`
/// if the string doesn't match that pattern.
fn parse_condition_str(cond: &str) -> Option<Condition> {
    let in_pos = cond.find(" in ")?;
    let parent = cond[..in_pos].trim().to_string();
    let vals_open = cond.find('{')?;
    let vals_close = cond.rfind('}')?;
    let allowed_values: Vec<String> = cond[vals_open + 1..vals_close]
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if parent.is_empty() || allowed_values.is_empty() { return None; }
    Some(Condition { parent, allowed_values })
}

fn strip_comment(line: &str) -> &str {
    match line.find('#') {
        Some(idx) => &line[..idx],
        None => line,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAPS: &str = "\
alpha {1.01, 1.066, 1.126, 1.189, 1.256, 1.326, 1.4} [1.189] #comment
rho {0, 0.17, 0.333, 0.5, 0.666, 0.83, 1} [0.5]
ps {0, 0.033, 0.066, 0.1, 0.133, 0.166, 0.2} [0.1]
wp {0, 0.01, 0.02, 0.03, 0.04, 0.05, 0.06} [0.03]
";

    const WITH_COND_AND_FORBIDDEN: &str = "\
mode {fast, slow} [fast]
thresh {1, 2, 3} [2] | mode in {slow}
{mode=fast, thresh=1}
";

    fn parse(text: &str) -> ParamSpace {
        let path = {
            use std::io::Write;
            let mut f = tempfile::NamedTempFile::new().unwrap();
            f.write_all(text.as_bytes()).unwrap();
            f.into_temp_path()
        };
        ParamSpace::from_file(path.to_str().unwrap()).unwrap()
    }

    #[test]
    fn saps_params() {
        let space = parse(SAPS);
        assert_eq!(space.params.len(), 4);
        let alpha = &space.params[0];
        assert_eq!(alpha.name, "alpha");
        assert_eq!(alpha.domain.len(), 7);
        assert_eq!(alpha.default, "1.189");
        assert!(alpha.condition.is_none());
        assert!(space.forbidden.is_empty());
    }

    #[test]
    fn conditional_param() {
        let space = parse(WITH_COND_AND_FORBIDDEN);
        let thresh = space.params.iter().find(|p| p.name == "thresh").unwrap();
        let cond = thresh.condition.as_ref().unwrap();
        assert_eq!(cond.parent, "mode");
        assert_eq!(cond.allowed_values, vec!["slow"]);
    }

    #[test]
    fn forbidden_combo() {
        let space = parse(WITH_COND_AND_FORBIDDEN);
        assert_eq!(space.forbidden.len(), 1);
        assert_eq!(space.forbidden[0], vec![
            ("mode".to_string(), "fast".to_string()),
            ("thresh".to_string(), "1".to_string()),
        ]);
    }

    #[test]
    fn active_params_conditional() {
        let space = parse(WITH_COND_AND_FORBIDDEN);
        let mut config: Config = [
            ("mode".to_string(), "fast".to_string()),
            ("thresh".to_string(), "2".to_string()),
        ].into();
        // thresh is only active when mode=slow
        let active = space.active_params(&config);
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].name, "mode");

        config.insert("mode".to_string(), "slow".to_string());
        let active = space.active_params(&config);
        assert_eq!(active.len(), 2);
    }

    #[test]
    fn is_forbidden_check() {
        let space = parse(WITH_COND_AND_FORBIDDEN);
        let config: Config = [
            ("mode".to_string(), "fast".to_string()),
            ("thresh".to_string(), "1".to_string()),
        ].into();
        assert!(space.is_forbidden(&config));

        let config2: Config = [
            ("mode".to_string(), "slow".to_string()),
            ("thresh".to_string(), "1".to_string()),
        ].into();
        assert!(!space.is_forbidden(&config2));
    }

    #[test]
    fn default_config() {
        let space = parse(SAPS);
        let cfg = space.default_config();
        assert_eq!(cfg["alpha"], "1.189");
        assert_eq!(cfg["rho"], "0.5");
    }
}

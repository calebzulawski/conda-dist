use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, bail};
use serde::Deserialize;

use super::{CondaDistConfig, DependencySpec};

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct ComponentConfig {
    pub dependencies: DependencySpec,
    pub includes: BTreeSet<String>,
    #[serde(skip)]
    resolved: BTreeSet<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct SubsetConfig {
    pub components: BTreeSet<String>,
    #[serde(skip)]
    resolved: BTreeSet<String>,
}

impl CondaDistConfig {
    pub(super) fn validate_components(&self) -> Result<()> {
        for (name, kind) in self
            .components
            .keys()
            .map(|name| (name, "component"))
            .chain(self.subsets.keys().map(|name| (name, "subset")))
        {
            if name.is_empty()
                || !name.starts_with(|ch: char| ch.is_ascii_lowercase() || ch.is_ascii_digit())
                || !name
                    .chars()
                    .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
                || name.starts_with("conda-")
                || name == "all"
                || name == "none"
            {
                bail!(
                    "{kind} '{name}' must use lowercase letters, digits and hyphens, start with a letter or digit, must not begin with 'conda-', and must not be 'all' or 'none'"
                );
            }
        }
        for (name, subset) in &self.subsets {
            if self.components.contains_key(name) {
                bail!("'{name}' is defined as both a component and a subset");
            }
            for component in &subset.components {
                if !self.components.contains_key(component) {
                    bail!("unknown component '{component}' in subset '{name}'");
                }
            }
        }
        Ok(())
    }
}

impl ComponentConfig {
    /// This component and all transitively included components, resolved at manifest load.
    pub fn resolved_components(&self) -> &BTreeSet<String> {
        &self.resolved
    }
}

impl SubsetConfig {
    /// All components included in this artifact collection, including group members.
    pub fn resolved_components(&self) -> &BTreeSet<String> {
        &self.resolved
    }
}

impl CondaDistConfig {
    pub(super) fn resolve_components(&mut self) -> Result<()> {
        let resolved = resolve_includes(&self.components)?;
        for (name, component) in &mut self.components {
            component.resolved = resolved[name].clone();
        }
        for subset in self.subsets.values_mut() {
            subset.resolved = subset
                .components
                .iter()
                .flat_map(|name| self.components[name].resolved_components().iter().cloned())
                .collect();
        }
        Ok(())
    }
}

fn resolve_includes(
    components: &BTreeMap<String, ComponentConfig>,
) -> Result<BTreeMap<String, BTreeSet<String>>> {
    fn visit(
        name: &str,
        components: &BTreeMap<String, ComponentConfig>,
        resolved: &mut BTreeMap<String, BTreeSet<String>>,
        path: &mut Vec<String>,
    ) -> Result<BTreeSet<String>> {
        if let Some(includes) = resolved.get(name) {
            return Ok(includes.clone());
        }
        if path.iter().any(|entry| entry == name) {
            path.push(name.to_string());
            bail!("component inclusion cycle: {}", path.join(" -> "));
        }
        let Some(component) = components.get(name) else {
            bail!(
                "unknown component '{name}' included by '{}'",
                path.last().unwrap()
            );
        };
        path.push(name.to_string());
        let mut includes = BTreeSet::from([name.to_string()]);
        for child in &component.includes {
            includes.extend(visit(child, components, resolved, path)?);
        }
        path.pop();
        resolved.insert(name.to_string(), includes.clone());
        Ok(includes)
    }
    let mut resolved = BTreeMap::new();
    for name in components.keys() {
        visit(name, components, &mut resolved, &mut Vec::new())?;
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_nested_overlapping_groups() {
        let components = toml::from_str(
            r#"
            [developer]
            includes = ["tools", "debug"]
            [tools]
            includes = ["debug"]
            [debug]
        "#,
        )
        .unwrap();
        let resolved = resolve_includes(&components).unwrap();
        assert_eq!(
            resolved["developer"],
            BTreeSet::from(["developer".into(), "tools".into(), "debug".into()])
        );
        assert_eq!(
            resolved["tools"],
            BTreeSet::from(["tools".into(), "debug".into()])
        );
    }

    #[test]
    fn rejects_cycles_and_missing_members() {
        for (source, expected) in [
            (
                "[tools]\nincludes = ['tools']",
                "component inclusion cycle: tools -> tools",
            ),
            (
                "[debug]\nincludes = ['tools']\n[tools]\nincludes = ['debug']",
                "component inclusion cycle: debug -> tools -> debug",
            ),
            (
                "[tools]\nincludes = ['missing']",
                "unknown component 'missing' included by 'tools'",
            ),
        ] {
            let components = toml::from_str(source).unwrap();
            assert_eq!(
                resolve_includes(&components).unwrap_err().to_string(),
                expected
            );
        }
    }
}

use std::collections::BTreeMap;
use std::sync::LazyLock;

use serde::Deserialize;

use super::grammar::{self, Grammar, Slot};
use super::{is_valid_token_value, ThemeResult, TokenMap, MAX_VALUE_LENGTH};

pub const THEME_PARTS_CATALOG_JSON: &str = include_str!("theme-parts.json");

pub const PART_TOKEN_PREFIX: &str = "--part-";

pub type PartsMap = BTreeMap<String, BTreeMap<String, String>>;

#[derive(Debug, Deserialize)]
pub struct PartsCatalog {
    pub properties: BTreeMap<String, PartProperty>,
    pub protected: Vec<ProtectedSurface>,
    pub parts: Vec<Part>,
    pub aliases: Vec<PartAlias>,
}

#[derive(Debug, Deserialize)]
pub struct PartProperty {
    #[serde(flatten)]
    pub grammar: Grammar,
    pub inherits: bool,
    #[serde(default)]
    pub effect: bool,
    #[serde(default, rename = "protectedValue")]
    pub protected_value: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ProtectedSurface {
    pub id: String,
}

#[derive(Debug, Deserialize)]
pub struct Part {
    pub id: String,
    pub selector: String,
    #[serde(default)]
    pub parent: Option<String>,
    pub frames: bool,
    pub properties: Vec<PartPropertyUse>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PartPropertyUse {
    pub name: String,
    pub token: String,
    pub default: String,
    #[serde(default)]
    pub inset_only: bool,
}

#[derive(Debug, Deserialize)]
pub struct PartAlias {
    pub token: String,
    pub property: String,
    pub parts: Vec<String>,
}

pub static CATALOG: LazyLock<PartsCatalog> = LazyLock::new(|| {
    serde_json::from_str(THEME_PARTS_CATALOG_JSON).expect("theme-parts.json is valid")
});

impl PartsCatalog {
    pub fn part(&self, id: &str) -> Option<&Part> {
        self.parts.iter().find(|p| p.id == id)
    }

    pub fn tokens(&self) -> impl Iterator<Item = (&Part, &PartPropertyUse)> {
        self.parts
            .iter()
            .flat_map(|part| part.properties.iter().map(move |p| (part, p)))
    }

    pub fn property(&self, name: &str) -> &PartProperty {
        self.properties
            .get(name)
            .expect("a catalog test checks that every part property is declared")
    }

    pub fn alias(&self, token: &str) -> Option<&PartAlias> {
        self.aliases.iter().find(|a| a.token == token)
    }
}

impl Part {
    pub fn property(&self, name: &str) -> Option<&PartPropertyUse> {
        self.properties.iter().find(|p| p.name == name)
    }
}

pub fn part_token(part: &str, property: &str) -> String {
    format!("{PART_TOKEN_PREFIX}{part}-{property}")
}

fn compile(part: &Part, usage: &PartPropertyUse, value: &str) -> Result<String, String> {
    if !is_valid_token_value(value) {
        return Err(format!(
            "the value is empty, longer than {MAX_VALUE_LENGTH} characters, or uses a banned form (url(), ;, braces, @, backslash, a comment)."
        ));
    }
    let is_colour_token: &dyn Fn(&str) -> bool = if part.frames {
        &super::is_frame_colour_token
    } else {
        &super::is_colour_token
    };
    let slot = Slot {
        property: &usage.name,
        part: &part.id,
        grammar: &CATALOG.property(&usage.name).grammar,
        inset_only: usage.inset_only,
        is_colour_token,
        frames: part.frames,
    };
    let canonical = grammar::canonical(&slot, value)?;
    if canonical.encode_utf16().count() > MAX_VALUE_LENGTH {
        return Err(format!(
            "the value is longer than {MAX_VALUE_LENGTH} characters once written out in full."
        ));
    }
    Ok(canonical)
}

fn alias_targets(alias: &PartAlias) -> impl Iterator<Item = (&Part, &PartPropertyUse)> {
    alias.parts.iter().filter_map(|id| {
        let part = CATALOG.part(id)?;
        Some((part, part.property(&alias.property)?))
    })
}

pub fn check_alias(alias: &PartAlias, value: &str) -> Result<(), String> {
    for (part, usage) in alias_targets(alias) {
        compile(part, usage, value)?;
    }
    Ok(())
}

pub fn validate_parts(label: &str, parts: &PartsMap) -> ThemeResult<()> {
    for (id, properties) in parts {
        let Some(part) = CATALOG.part(id) else {
            let why = if CATALOG.protected.iter().any(|p| p.id == *id) {
                "a protected surface, so a theme cannot style it."
            } else {
                "not a part. GET /api/v1/themes/parts lists them."
            };
            return Err(format!("{label}.{id}: {why}").into());
        };
        for (property, value) in properties {
            let Some(usage) = part.property(property) else {
                let names: Vec<&str> = part.properties.iter().map(|p| p.name.as_str()).collect();
                return Err(
                    format!("{label}.{id}.{property}: {id} takes {}.", names.join(", ")).into(),
                );
            };
            compile(part, usage, value).map_err(|why| format!("{label}.{id}.{property}: {why}"))?;
        }
    }
    Ok(())
}

pub fn resolve(shared: &PartsMap, mode: Option<&PartsMap>, tokens: &TokenMap) -> TokenMap {
    let mut merged = shared.clone();
    for (id, properties) in mode.into_iter().flatten() {
        let entry = merged.entry(id.clone()).or_default();
        entry.extend(properties.iter().map(|(k, v)| (k.clone(), v.clone())));
    }
    let mut out = TokenMap::new();
    for alias in &CATALOG.aliases {
        let Some(value) = tokens.get(&alias.token) else {
            continue;
        };
        for (part, usage) in alias_targets(alias) {
            if let Ok(canonical) = compile(part, usage, value) {
                out.insert(usage.token.clone(), canonical);
            }
        }
    }
    for (id, properties) in &merged {
        let Some(part) = CATALOG.part(id) else {
            continue;
        };
        for (property, value) in properties {
            let Some(usage) = part.property(property) else {
                continue;
            };
            if let Ok(canonical) = compile(part, usage, value) {
                out.insert(usage.token.clone(), canonical);
            }
        }
    }
    out
}

pub fn check_part_token(name: &str, value: &str) -> Result<String, String> {
    let Some((part, usage)) = CATALOG.tokens().find(|(_, usage)| usage.token == name) else {
        return Err("not a part token. GET /api/v1/themes/parts lists them.".into());
    };
    compile(part, usage, value)
}

pub fn check_shape(label: &str, value: &serde_json::Value) -> ThemeResult<()> {
    let Some(parts) = value.as_object() else {
        return Err(format!("{label}: must be a map of part ids to properties.").into());
    };
    for (id, properties) in parts {
        let Some(properties) = properties.as_object() else {
            return Err(format!("{label}.{id}: must be a map of properties to values.").into());
        };
        if let Some((property, _)) = properties.iter().find(|(_, v)| !v.is_string()) {
            return Err(format!("{label}.{id}.{property}: must be a string.").into());
        }
    }
    Ok(())
}

#[path = "parts_codegen.rs"]
pub mod codegen;

#[cfg(test)]
#[path = "parts_tests.rs"]
mod tests;

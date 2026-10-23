//! Offline validation of the version-pinned Jellyfin HTTP controller attribute inventory.
//! The matrix is NOT proof of runtime interoperability or implemented endpoints.
use std::{collections::HashSet, fs};

use serde_json::Value;

const PINNED_COMMIT: &str = "06de749d435e2d0be0c5119a41f2300fdd7143e5";
const CONTROLLERS: usize = 60;
const HTTP_ATTRIBUTES: usize = 419;

fn validate(doc: &Value) -> Result<(usize, usize), String> {
    if doc["schema"] != 1
        || doc["baseline"]["repository"] != "jellyfin/jellyfin"
        || doc["baseline"]["tag"] != "v12.3"
        || doc["baseline"]["commit"] != PINNED_COMMIT
        || doc["expected_controller_count"] != CONTROLLERS
    {
        return Err("unexpected Jellyfin API baseline".into());
    }
    let controllers = doc["controllers"]
        .as_array()
        .ok_or("missing controller inventory")?;
    if controllers.len() != CONTROLLERS {
        return Err(format!(
            "expected {CONTROLLERS} controllers, got {}",
            controllers.len()
        ));
    }

    let mut controllers_seen = HashSet::new();
    let mut actions = 0;
    for c in controllers {
        let name = c["controller"].as_str().ok_or("missing controller name")?;
        if !name.ends_with("Controller") || !controllers_seen.insert(name) {
            return Err(format!("duplicate or malformed controller: {name}"));
        }
        let expected_path = format!("Jellyfin.Api/Controllers/{name}.cs");
        if c["source_path"] != expected_path {
            return Err(format!("{name}: source path mismatch"));
        }
        let sha = c["upstream_blob_sha"].as_str().ok_or("missing blob SHA")?;
        if sha.len() != 40 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(format!("{name}: malformed upstream blob SHA"));
        }
        c["route_prefix"]
            .as_str()
            .ok_or_else(|| format!("{name}: missing route prefix"))?;
        let endpoints = c["actions"]
            .as_array()
            .ok_or_else(|| format!("{name}: missing actions"))?;
        if endpoints.is_empty() {
            return Err(format!("{name}: empty controller actions"));
        }
        let mut source_lines = HashSet::new();
        for a in endpoints {
            let method = a["method"].as_str().ok_or("missing HTTP method")?;
            if !matches!(
                method,
                "GET" | "POST" | "PUT" | "DELETE" | "PATCH" | "HEAD" | "OPTIONS"
            ) {
                return Err(format!("{name}: unsupported HTTP verb"));
            }
            let line = a["source_line"].as_u64().ok_or("missing source line")?;
            if line == 0 || !source_lines.insert(line) {
                return Err(format!(
                    "{name}: duplicate or invalid attribute source line"
                ));
            }
            match a["path"].as_str() {
                Some(path) if path.starts_with('/') && !path.contains("//") => {}
                None if a["unresolved_attribute"].is_string() => {}
                _ => return Err(format!("{name}: bad or unreviewed path field")),
            }
            actions += 1;
        }
    }
    if actions != HTTP_ATTRIBUTES {
        return Err(format!(
            "expected {HTTP_ATTRIBUTES} method attributes; found {actions}"
        ));
    }
    Ok((controllers.len(), actions))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if !matches!(std::env::args().nth(1).as_deref(), None | Some("--check")) {
        return Err("usage: pelagos-api-matrix [--check]".into());
    }
    let doc: Value = serde_json::from_str(&fs::read_to_string("docs/api-contracts.json")?)?;
    let (controllers, attributes) = validate(&doc)?;
    println!(
        "Pelagos pinned API controller attributes: {controllers} controllers, {attributes} HTTP method attributes (not accepted parity)"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_attributes_are_valid() {
        let doc: Value =
            serde_json::from_str(include_str!("../../docs/api-contracts.json")).unwrap();
        assert_eq!(validate(&doc).unwrap(), (60, 419));
    }

    #[test]
    fn removing_a_controller_fails() {
        let mut doc: Value =
            serde_json::from_str(include_str!("../../docs/api-contracts.json")).unwrap();
        doc["controllers"].as_array_mut().unwrap().pop();
        assert!(validate(&doc).is_err());
    }

    #[test]
    fn forged_attribute_path_fails() {
        let mut doc: Value =
            serde_json::from_str(include_str!("../../docs/api-contracts.json")).unwrap();
        doc["controllers"][0]["actions"][0]["path"] = Value::String("bad/path".to_string());
        assert!(validate(&doc).is_err());
    }
}

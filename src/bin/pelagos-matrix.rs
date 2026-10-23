//! Verifies the pinned full C# file-level port ledger without downloading Jellyfin.
use std::{collections::HashSet, fs, path::Path};

use serde_json::Value;

fn validate(matrix: &Value) -> Result<(usize, usize, usize), String> {
    if matrix["schema"] != 1
        || matrix["source"] != "jellyfin/jellyfin"
        || matrix["tag"] != "v12.3"
        || matrix["commit"] != "06de749d435e2d0be0c5119a41f2300fdd7143e5"
        || matrix["tree"] != "5d01346d1cc857fc7c3c4d581205eea95dda3c0d"
    {
        return Err("upstream baseline differs from pinned v12.3".into());
    }
    let entries = matrix["files"]
        .as_array()
        .ok_or("missing C# source file array")?;
    let mut seen = HashSet::new();
    let mut server = 0;
    let mut partial = 0;
    let mut accepted = 0;
    let mut references = 0;
    for entry in entries {
        let path = entry["source_path"].as_str().ok_or("missing source_path")?;
        let sha = entry["upstream_blob_sha"]
            .as_str()
            .ok_or("missing upstream blob SHA")?;
        let role = entry["role"].as_str().ok_or("missing role")?;
        let status = entry["status"].as_str().ok_or("missing status")?;
        let replacements = entry["rust_paths"].as_array().ok_or("missing rust_paths")?;
        if !path.ends_with(".cs")
            || Path::new(path).is_absolute()
            || path.split('/').any(|part| part == "..")
            || !seen.insert(path)
            || sha.len() != 40
            || !sha.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(format!("bad or duplicate source identity: {path}"));
        }
        for replacement in replacements {
            let rust = replacement.as_str().ok_or("non-string Rust replacement")?;
            if rust.is_empty()
                || Path::new(rust).is_absolute()
                || rust.split('/').any(|part| part == "..")
                || !Path::new(rust).is_file()
            {
                return Err(format!("{path}: invalid local Rust replacement {rust}"));
            }
        }
        match role {
            "server_source" => {
                server += 1;
                match status {
                    "unported" => {
                        if !replacements.is_empty() {
                            return Err(format!("{path}: unported source has replacements"));
                        }
                    }
                    "partial" => {
                        if replacements.is_empty() {
                            return Err(format!("{path}: partial source has no replacement"));
                        }
                        partial += 1;
                    }
                    "accepted" => {
                        let evidence = entry["evidence"].as_array().ok_or("missing evidence")?;
                        if replacements.is_empty()
                            || evidence.len() < 2
                            || evidence
                                .iter()
                                .any(|e| e.as_str().is_none_or(str::is_empty))
                        {
                            return Err(format!("{path}: unverified accepted parity"));
                        }
                        accepted += 1;
                    }
                    _ => return Err(format!("{path}: unknown source status")),
                }
            }
            "reference_test" | "reference_fuzz" => {
                references += 1;
                if status != "reference_only" || !replacements.is_empty() {
                    return Err(format!("{path}: reference source falsely marked ported"));
                }
            }
            _ => return Err(format!("{path}: unknown source role")),
        }
    }
    if entries.len() != matrix["file_count"].as_u64().unwrap_or_default() as usize
        || server != matrix["server_source_count"].as_u64().unwrap_or_default() as usize
        || references != matrix["reference_count"].as_u64().unwrap_or_default() as usize
    {
        return Err("catalog counts do not match file entries".into());
    }
    Ok((server, partial, accepted))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if !matches!(std::env::args().nth(1).as_deref(), None | Some("--check")) {
        return Err("usage: pelagos-matrix [--check]".into());
    }
    let data = fs::read_to_string("docs/port-matrix.json")?;
    let value: Value = serde_json::from_str(&data)?;
    let (server, partial, accepted) = validate(&value)?;
    println!(
        "Pelagos C# source inventory: {accepted}/{server} accepted, {partial} partial; {} reference files",
        value["reference_count"]
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_tree_inventory_is_consistent() {
        let matrix: Value =
            serde_json::from_str(include_str!("../../docs/port-matrix.json")).unwrap();
        assert_eq!(validate(&matrix).unwrap(), (1932, 2, 0));
    }

    #[test]
    fn fake_acceptance_is_rejected() {
        let mut matrix: Value =
            serde_json::from_str(include_str!("../../docs/port-matrix.json")).unwrap();
        let entry = matrix["files"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|e| e["status"] == "partial")
            .unwrap();
        entry["status"] = Value::String("accepted".to_string());
        assert!(validate(&matrix).is_err());
    }
}

use std::fs;
use std::path::{Path, PathBuf};
use toml::Value;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let lang_dir = Path::new("assets/default/lang");
    
    // Read the reference en-US.toml file
    let en_us_path = lang_dir.join("en-US.toml");
    let en_us_content = fs::read_to_string(&en_us_path)?;
    let en_us_value: Value = toml::from_str(&en_us_content)?;
    
    println!("Loaded reference file: en-US.toml");
    
    // Extract all keys from the reference file
    let reference_keys = extract_all_keys(&en_us_value, String::new());
    println!("Found {} keys in reference file", reference_keys.len());
    
    // Process all other .toml files in the directory
    let entries = fs::read_dir(lang_dir)?;
    let mut processed_count = 0;
    
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        
        if path.extension().map_or(false, |ext| ext == "toml") {
            let filename = path.file_name().unwrap().to_string_lossy();
            
            // Skip the reference file
            if filename == "en-US.toml" {
                continue;
            }
            
            println!("Processing: {}", filename);
            process_lang_file(&path, &reference_keys)?;
            processed_count += 1;
        }
    }
    
    if processed_count == 0 {
        println!("No other language files found to process.");
    } else {
        println!("Successfully processed {} language files.", processed_count);
    }
    
    Ok(())
}

fn extract_all_keys(value: &Value, prefix: String) -> Vec<String> {
    let mut keys = Vec::new();
    
    match value {
        Value::Table(table) => {
            for (key, val) in table {
                let full_key = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{}.{}", prefix, key)
                };
                
                match val {
                    Value::Table(_) => {
                        // Recursively extract keys from nested tables
                        keys.extend(extract_all_keys(val, full_key));
                    }
                    _ => {
                        // This is a leaf value, add the key
                        keys.push(full_key);
                    }
                }
            }
        }
        _ => {}
    }
    
    keys
}

fn process_lang_file(path: &PathBuf, reference_keys: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    // Read the existing file
    let content = fs::read_to_string(path)?;
    let mut value: Value = toml::from_str(&content)?;
    
    let mut missing_keys = Vec::new();
    let mut added_count = 0;
    
    // Check each reference key
    for key in reference_keys {
        if !key_exists(&value, key) {
            missing_keys.push(key.clone());
        }
    }
    
    // Add missing keys
    for key in missing_keys {
        let missing_value = format!("MISSING {}", key);
        set_nested_key(&mut value, &key, Value::String(missing_value));
        added_count += 1;
        println!("  Added missing key: {}", key);
    }
    
    if added_count > 0 {
        // Write the updated file back
        let updated_content = toml::to_string_pretty(&value)?;
        fs::write(path, updated_content)?;
        println!("  Updated {} with {} missing keys", path.file_name().unwrap().to_string_lossy(), added_count);
    } else {
        println!("  No missing keys found in {}", path.file_name().unwrap().to_string_lossy());
    }
    
    Ok(())
}

fn key_exists(value: &Value, key: &str) -> bool {
    let parts: Vec<&str> = key.split('.').collect();
    let mut current = value;
    
    for part in parts {
        match current {
            Value::Table(table) => {
                if let Some(next_value) = table.get(part) {
                    current = next_value;
                } else {
                    return false;
                }
            }
            _ => return false,
        }
    }
    
    true
}

fn set_nested_key(value: &mut Value, key: &str, new_value: Value) {
    let parts: Vec<&str> = key.split('.').collect();
    let mut current = value;
    
    // Navigate to the parent of the target key, creating tables as needed
    for part in &parts[..parts.len() - 1] {
        if let Value::Table(table) = current {
            let entry = table.entry(part.to_string()).or_insert_with(|| Value::Table(toml::map::Map::new()));
            current = entry;
        }
    }
    
    // Set the final key
    if let Value::Table(table) = current {
        table.insert(parts[parts.len() - 1].to_string(), new_value);
    }
}

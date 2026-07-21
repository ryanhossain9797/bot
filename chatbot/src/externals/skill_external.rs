use std::path::PathBuf;
use tokio::fs;

const SKILLS_DIR: &str = "/app/skills";

async fn skill_names() -> Result<Vec<String>, String> {
    let mut dir = fs::read_dir(SKILLS_DIR)
        .await
        .map_err(|e| format!("could not open the skills directory: {e}"))?;
    let mut names = Vec::new();
    while let Some(entry) = dir
        .next_entry()
        .await
        .map_err(|e| format!("could not read the skills directory: {e}"))?
    {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("md") {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                names.push(stem.to_string());
            }
        }
    }
    names.sort();
    Ok(names)
}

pub async fn list_skills() -> Result<String, String> {
    let names = skill_names().await?;
    if names.is_empty() {
        return Ok("No skills are available.".to_string());
    }
    let list = names
        .iter()
        .map(|n| format!("- {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    Ok(format!(
        "Available skills ({}). Call use_skill again with a name to read one in full:\n{list}",
        names.len()
    ))
}

pub async fn read_skill(name: &str) -> Result<String, String> {
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return Err(format!("'{name}' is not a valid skill name."));
    }
    let path = PathBuf::from(SKILLS_DIR).join(format!("{name}.md"));
    match fs::read_to_string(&path).await {
        Ok(content) => Ok(content),
        Err(_) => {
            let hint = match skill_names().await {
                Ok(names) if !names.is_empty() => format!("available skills: {}", names.join(", ")),
                _ => "no skills are available".to_string(),
            };
            Err(format!("no skill named '{name}' ({hint})"))
        }
    }
}

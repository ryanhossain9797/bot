use crate::machine::{EntityId, StateMachine};
use anyhow::Context;
use std::path::{Path, PathBuf};

// Sole owner of on-disk entity state. Layout: framework_db/<state machine>/<entity id>.json.
// Nothing outside this module touches the filesystem.

fn entity_path<SM: StateMachine>(id: &SM::Id) -> PathBuf {
    let safe_id: String = id
        .get_id_string()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '_' })
        .collect();
    Path::new("framework_db").join(SM::name()).join(format!("{safe_id}.json"))
}

// Write the state atomically (temp file + rename). Returns Err so the caller can abort.
pub(crate) fn write<SM: StateMachine>(id: &SM::Id, state: &SM::State) -> anyhow::Result<()> {
    let path = entity_path::<SM>(id);
    let dir = path.parent().expect("entity path always has a parent");
    std::fs::create_dir_all(dir).with_context(|| format!("create_dir_all {}", dir.display()))?;
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(state).context("serialize state")?;
    std::fs::write(&tmp, &bytes).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("rename to {}", path.display()))?;
    Ok(())
}

// Read persisted state. Missing, unreadable, or stale/corrupt (schema drift) all return None,
// so the caller falls back to fresh construction rather than failing.
pub(crate) fn read<SM: StateMachine>(id: &SM::Id) -> Option<SM::State> {
    let path = entity_path::<SM>(id);
    let bytes = std::fs::read(&path).ok()?;
    match serde_json::from_slice(&bytes) {
        Ok(state) => Some(state),
        Err(e) => {
            eprintln!(
                "[store] deserialize {} failed: {e}; falling back to construct",
                path.display()
            );
            None
        }
    }
}

// Remove the persisted file (best-effort). A missing file is not an error.
// Not wired into the handle yet — entity deletion semantics aren't defined.
#[allow(dead_code)]
pub(crate) fn delete<SM: StateMachine>(id: &SM::Id) {
    let path = entity_path::<SM>(id);
    if let Err(e) = std::fs::remove_file(&path) {
        if e.kind() != std::io::ErrorKind::NotFound {
            eprintln!("[store] remove {} failed: {e}", path.display());
        }
    }
}

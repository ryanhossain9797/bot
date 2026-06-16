use crate::machine::{EntityId, StateMachine};
use anyhow::Context;

fn entity_path<SM: StateMachine>(id: &SM::Id) -> std::path::PathBuf {
    let safe_id: String = id
        .get_id_string()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '_' })
        .collect();
    std::path::Path::new("framework_db")
        .join(SM::name())
        .join(format!("{safe_id}.json"))
}

pub(crate) fn persist_state<SM: StateMachine>(id: &SM::Id, state: &SM::State) -> anyhow::Result<()> {
    let path = entity_path::<SM>(id);
    let dir = path.parent().expect("entity path always has a parent");
    std::fs::create_dir_all(dir).with_context(|| format!("create_dir_all {}", dir.display()))?;
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(state).context("serialize state")?;
    std::fs::write(&tmp, &bytes).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("rename to {}", path.display()))?;
    Ok(())
}

pub(crate) fn load_state<SM: StateMachine>(id: &SM::Id) -> Option<SM::State> {
    let bytes = std::fs::read(entity_path::<SM>(id)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub(crate) fn delete_state<SM: StateMachine>(id: &SM::Id) -> anyhow::Result<()> {
    let path = entity_path::<SM>(id);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("remove {}", path.display())),
    }
}

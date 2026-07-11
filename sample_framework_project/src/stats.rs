//! Second state-machine type: a singleton that counts handled messages.
//! Exists to exercise entity→entity messaging (conversations enqueue actions to it)
//! and the framework's design-for-N-machine-types goal.

use re_framework::{Effects, EntityId, Identified, Scheduled, StateMachine};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct StatsId;

impl EntityId for StatsId {
    fn get_id_string(&self) -> String {
        "global".to_string()
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Stats {
    total: u64,
    per_conversation: BTreeMap<String, u64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum StatsAction {
    MessageHandled { conversation: String },
}

#[derive(Serialize, Deserialize)]
pub struct StatsInit {
    pub id: StatsId,
}

impl Identified for StatsInit {
    type Id = StatsId;
    fn get_id(&self) -> &StatsId {
        &self.id
    }
}

pub struct StatsMachine;

impl StateMachine for StatsMachine {
    type State = Stats;
    type Id = StatsId;
    type Action = StatsAction;
    type Construction = StatsInit;
    type Env = ();

    fn construct(_init: StatsInit, _effects: &mut Effects<Self>) -> Stats {
        Stats {
            total: 0,
            per_conversation: BTreeMap::new(),
        }
    }

    fn transition(
        state: &Stats,
        _id: &StatsId,
        _env: &Arc<()>,
        action: &StatsAction,
        _effects: &mut Effects<Self>,
    ) -> anyhow::Result<Stats> {
        let StatsAction::MessageHandled { conversation } = action;
        let per_conversation = state
            .per_conversation
            .iter()
            .map(|(conv, count)| (conv.clone(), *count))
            .chain([(
                conversation.clone(),
                state.per_conversation.get(conversation).copied().unwrap_or(0) + 1,
            )])
            .collect::<BTreeMap<_, _>>();
        let next = Stats {
            total: state.total + 1,
            per_conversation,
        };
        println!(
            "[stats] {} handled total ({} from {})",
            next.total,
            next.per_conversation
                .get(conversation)
                .copied()
                .unwrap_or(0),
            conversation
        );
        Ok(next)
    }

    fn schedule(_state: &Stats) -> Option<Scheduled<StatsAction>> {
        None
    }

    fn name() -> &'static str {
        "StatsMachine"
    }
}

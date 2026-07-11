//! Deterministic harness for re_framework (#186): a minimal chatbot replica with a
//! fake brain and fake tool instead of an LLM. Runs natively — no containers, no GPU.
//! State persists to ./framework_db (relative to the working directory).

mod conversation;
mod externals;
mod stats;

use conversation::{ConversationAction, ConversationId, ConversationInit, ConversationMachine};
use re_framework::StateMachine;
use stats::{StatsId, StatsInit, StatsMachine};
use tokio::io::{AsyncBufReadExt, BufReader};

const BANNER: &str = "\
sample_framework_project — deterministic re_framework harness (no LLM, no containers)
  <conv_id>: <text>    send <text> to conversation <conv_id> (constructed on first use)
  <text>               send to the default conversation `main`
  tool add <a> <b>     (as the text) makes the fake brain call the fake `add` tool
  exit                 quit
State persists to ./framework_db — kill and restart to watch conversations resume.
Idle conversations reset after 60s (persisted timer — survives a restart too).";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    conversation::init_conversation_machine();
    stats::init_stats_machine();
    StatsMachine::handle().maybe_construct(StatsInit { id: StatsId });

    println!("{BANNER}");

    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    while let Some(line) = lines.next_line().await? {
        match line.trim() {
            "" => {}
            "exit" => break,
            line => {
                let (conv, text) = line
                    .split_once(':')
                    .map(|(conv, text)| (conv.trim(), text.trim()))
                    .unwrap_or(("main", line));
                let id = ConversationId(conv.to_string());
                ConversationMachine::handle().maybe_construct(ConversationInit { id: id.clone() });
                ConversationMachine::handle().act(id, ConversationAction::UserMessage(text.to_string()));
            }
        }
    }

    // grace so in-flight effect chains (decide → tool → reply) settle before the runtime drops
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    Ok(())
}

//! **Live probes of the API** — what no fake server can say: whether each
//! model takes the request Shannon builds for it, and what the API does with
//! thinking across a tool round and after an edit (Steven, 10-31: "I think
//! this is more than low enough that we can automate").
//!
//! It SPENDS: cents a run. The key is read by this process from the file
//! `SHANNON_API_KEY_FILE` names, and never printed; what is printed is each
//! request's model and shape, its outcome, its token counts, and the API's
//! own error text.
//!
//! ```text
//! SHANNON_API_KEY_FILE=.test_key.api cargo run -p shannon-core --example live_probe -- models
//! SHANNON_API_KEY_FILE=.test_key.api cargo run -p shannon-core --example live_probe -- thinking [effort]
//! ```
//!
//! - `models`: every row of the alias table but the stub, asked one word at
//!   its own reply ceiling, with the thinking Shannon asks of its category
//!   (a budgeted model a 2048 budget) — a wrong ceiling or a thinking shape
//!   the model refuses is a 400 here, not in a session.
//! - `thinking`: Sonnet 5.5 runs one command, its thinking sent back on the
//!   next round (nothing should be dropped); then an earlier message is
//!   edited and the turn asked again (each block sent should be dropped,
//!   the conversation before it having changed). A round that thinks
//!   nothing makes the probe say it proved nothing.

use shannon_core::backend::anthropic::AnthropicApi;
use shannon_core::backend::models::{self, ThinkingKind};
use shannon_core::config::CacheConfig;
use shannon_core::{
    Backend, Block, Conversation, Dropped, Effort, Message, Role, SendRequest, StopReason,
    StreamMsg, ThinkingAsk, ThinkingDisplay, ToolDef, Usage,
};

/// What one request was heard to say.
#[derive(Default)]
struct Heard {
    text: String,
    thoughts: Vec<Block>,
    calls: Vec<Block>,
    dropped: Vec<Dropped>,
    usage: Option<Usage>,
    stop: Option<StopReason>,
    error: Option<String>,
}

impl Heard {
    /// One line: how it ended, and what it cost in tokens.
    fn outcome(&self) -> String {
        let tokens = self.usage.map_or(String::new(), |u| {
            format!(
                " — in {} (cache read {}, written {}), out {}",
                u.input_tokens,
                u.cache_read_input_tokens,
                u.cache_creation_input_tokens,
                u.output_tokens
            )
        });
        match &self.error {
            Some(why) => format!("ERROR {why}"),
            None => format!(
                "ok, stop {:?}, {} thinking block(s), {} call(s){tokens}",
                self.stop,
                self.thoughts.len(),
                self.calls.len()
            ),
        }
    }
}

/// Send `req` and hear it out, feeding what streams into `turn` when given
/// one — as the app does, so the next request is built as the app builds it.
async fn hear(api: &AnthropicApi, req: SendRequest, mut turn: Option<&mut Conversation>) -> Heard {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let task = api.send_message(req, tx);
    let mut heard = Heard::default();
    while let Some(msg) = rx.recv().await {
        match msg {
            StreamMsg::Chunk(text) => {
                if let Some(t) = turn.as_deref_mut() {
                    t.push_chunk(&text);
                }
                heard.text.push_str(&text);
            }
            StreamMsg::Thought(block) => {
                if let Some(t) = turn.as_deref_mut() {
                    t.push_thinking(block.clone());
                }
                heard.thoughts.push(block);
            }
            StreamMsg::ToolUse(block) => {
                if let Some(t) = turn.as_deref_mut() {
                    t.push_tool_use(block.clone());
                }
                heard.calls.push(block);
            }
            StreamMsg::ThinkingDropped(why) => heard.dropped.extend(why),
            StreamMsg::Usage(u) => heard.usage = Some(u),
            StreamMsg::Stopped(reason) => heard.stop = Some(reason),
            StreamMsg::Error(e) => {
                heard.error = Some(e.to_string());
                break;
            }
            StreamMsg::Complete => break,
            _ => {}
        }
    }
    let _ = task.await;
    heard
}

/// Every thinking block in `messages` — what a request sends back.
fn thinking_sent(messages: &[Message]) -> usize {
    messages
        .iter()
        .flat_map(|m| &m.blocks)
        .filter(|b| matches!(b, Block::Thinking { .. } | Block::RedactedThinking { .. }))
        .count()
}

/// `models`: each row asked one word, at its ceiling, thinking as Shannon
/// asks its category.
async fn probe_models(api: &AnthropicApi) {
    for row in models::ALIASES.iter().filter(|m| m.id != models::STUB) {
        let kind = models::thinking_kind(row.id);
        let mut req = SendRequest::new(vec![Message::user("Reply with the single word: ok")]);
        req.model = Some(row.id.to_string());
        req.thinking = ThinkingAsk {
            display: ThinkingDisplay::Summarized,
            budget: match kind {
                Some(ThinkingKind::Budget) => 2048,
                _ => 0,
            },
        };
        let heard = hear(api, req, None).await;
        println!(
            "{:<11} {:<27} max_tokens {:>6}  {:<9} {}",
            row.alias,
            row.id,
            models::max_output_tokens(row.id),
            format!("{kind:?}").replace("Some(", "").replace(')', ""),
            heard.outcome()
        );
    }
}

/// `thinking`: a tool round carried, then an edit.
async fn probe_thinking(api: &AnthropicApi, effort: Option<Effort>) {
    const MODEL: &str = "claude-sonnet-5-5";
    let ask = |messages: Vec<Message>| {
        let mut req = SendRequest::new(messages);
        req.model = Some(MODEL.to_string());
        req.tools = vec![ToolDef::bash()];
        req.effort = effort;
        req
    };
    let mut turn = Conversation::new();
    turn.begin_turn(
        "Use the bash tool to run `echo probe-7341`, then tell me exactly what it printed \
         and whether the number is prime. Work it out before you answer.",
    );

    // Round 1: thinking, then a call.
    let heard = hear(api, ask(turn.snapshot()), Some(&mut turn)).await;
    println!("round 1 (asks for the call):  {}", heard.outcome());
    if heard.error.is_some() || heard.calls.is_empty() {
        println!("no call to answer — the probe stops here");
        return;
    }
    let results = heard
        .calls
        .iter()
        .filter_map(|call| match call {
            Block::ToolUse { id, .. } => Some(Block::ToolResult {
                tool_use_id: id.clone(),
                content: "probe-7341\n".to_string(),
                is_error: false,
            }),
            _ => None,
        })
        .collect();
    turn.finish_tool_round(Message {
        role: Role::User,
        content: String::new(),
        blocks: results,
    });

    // Round 2: the call answered, round 1's thinking sent back unchanged.
    let messages = turn.snapshot();
    let sent = thinking_sent(&messages);
    let heard = hear(api, ask(messages), Some(&mut turn)).await;
    println!("round 2 (sends {sent} back):     {}", heard.outcome());
    println!("  dropped: {:?} — expected none", heard.dropped);
    turn.commit_turn_holding(&shannon_core::Aliases::default());

    // An edit: the first message changed, a new question after it — every
    // thinking block sent now follows a prefix that is not the one it was
    // made under.
    let mut messages = turn.snapshot();
    if let Some(first) = messages.first_mut() {
        first.content.push_str(" Be brief.");
    }
    messages.push(Message::user("Thanks. Which number was it?"));
    let sent = thinking_sent(&messages);
    let heard = hear(api, ask(messages), None).await;
    println!("after an edit (sends {sent}):  {}", heard.outcome());
    println!("  dropped: {:?} — expected one Prefix each", heard.dropped);
    if sent == 0 {
        println!("NOTHING THOUGHT: the probe proved nothing — run again with a higher effort");
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let mode = args.next().unwrap_or_default();
    let path = std::env::var_os(shannon_core::config::KEY_FILE_VAR)
        .ok_or_else(|| anyhow::anyhow!("set SHANNON_API_KEY_FILE to the key's file"))?;
    // Read here, held as a `Secret` from here on, never printed.
    let key = std::fs::read_to_string(&path)?.trim().to_string();
    let api = AnthropicApi::new(key, models::resolve("sonnet"), CacheConfig::default())?;
    match mode.as_str() {
        "models" => probe_models(&api).await,
        "thinking" => {
            let effort = match args.next().as_deref() {
                None => Some(Effort::High),
                Some(word) => Some(
                    Effort::ALL
                        .into_iter()
                        .find(|e| e.as_str() == word)
                        .ok_or_else(|| anyhow::anyhow!("no effort `{word}`"))?,
                ),
            };
            probe_thinking(&api, effort).await
        }
        _ => anyhow::bail!("say `models` or `thinking [effort]`"),
    }
    Ok(())
}

use crate::shared::kernel::message::ThinkingBlock;
use crate::shared::kernel::tool_call::ToolCall;

/// The assembled result of one streamed assistant turn: any answer text and the tool calls it
/// requested (assembled from their streamed fragments, ordered by index).
#[derive(Debug, Clone, PartialEq)]
pub struct CompletedTurn {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    /// The Anthropic `thinking` block (text + signature), when extended thinking is enabled. `None` for
    /// every other provider and for Anthropic with thinking off.
    pub thinking: Option<ThinkingBlock>,
}

pub mod anthropic;
pub mod factory;
pub mod http_error;
pub mod openai;
pub mod secrets;
pub(crate) mod tool_args;
pub mod unconfigured;

#[cfg(test)]
pub(crate) mod test_support;

/// Cap on the bytes a single streamed turn may accumulate (streamed content + tool-call arguments).
/// Provider responses are untrusted input, and `read_timeout` only bounds idle time between chunks (it
/// resets on each chunk), so a misbehaving provider streaming continuously could otherwise grow memory
/// without bound. Single-sourced here so both the OpenAI and Anthropic accumulators enforce one ceiling.
/// Generous — far above any real turn — purely a safety ceiling.
pub(crate) const MAX_STREAM_BYTES: usize = 8 * 1024 * 1024;

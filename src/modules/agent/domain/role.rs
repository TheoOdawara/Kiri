/// The author of a message. Pure domain: each variant's OpenAI wire string is mapped in the provider
/// DTO (`provider::infrastructure::openai::message_dto::wire_role`), keeping the domain serde-free.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

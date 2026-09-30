/// A semantic piece of the streamed completion: the model's reasoning, or its answer content.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    Reasoning(String),
    Content(String),
}

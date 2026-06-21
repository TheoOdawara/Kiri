use serde::Deserialize;

#[derive(Deserialize)]
pub struct PathArgs {
    pub path: String,
}

#[derive(Deserialize)]
pub struct WriteArgs {
    pub path: String,
    pub content: String,
}

#[derive(Deserialize)]
pub struct MoveArgs {
    pub source: String,
    pub destination: String,
}

#[derive(Deserialize)]
pub struct EditArgs {
    pub path: String,
    pub old_string: String,
    pub new_string: String,
}

#[derive(Deserialize)]
pub struct ListArgs {
    #[serde(default = "dot")]
    pub path: String,
}

#[derive(Deserialize)]
pub struct SearchArgs {
    pub query: String,
    #[serde(default = "dot")]
    pub path: String,
}

fn dot() -> String {
    ".".to_string()
}

pub fn parse<T: serde::de::DeserializeOwned>(args: &str) -> Result<T, serde_json::Error> {
    serde_json::from_str(args)
}

mod app;
mod modules;
mod shared;

#[cfg(test)]
mod characterization;

use crate::shared::infra::config::Settings;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let settings = Settings::load()?;
    app::wire(settings).await?.run().await
}

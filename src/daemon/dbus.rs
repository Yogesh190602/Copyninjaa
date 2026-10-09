use anyhow::Result;
use log::info;
use zbus::Connection;

struct CopyNinjaDaemon;

#[zbus::interface(name = "com.copyninja.Daemon")]
impl CopyNinjaDaemon {
    async fn new_entry(&self, text: String) {
        info!("D-Bus NewEntry received ({} chars)", text.len());
        super::blocking(move || crate::storage::process_text(&text)).await;
    }
}

pub async fn setup() -> Result<Connection> {
    let conn = zbus::connection::Builder::session()?
        .name("com.copyninja.Daemon")?
        .serve_at("/com/copyninja/Daemon", CopyNinjaDaemon)?
        .build()
        .await?;

    Ok(conn)
}

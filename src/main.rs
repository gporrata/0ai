mod a2a;
mod app;
mod db;
mod llm;
mod mcp;
mod ui;

use anyhow::Result;
use app::App;
use db::Database;
use tracing_subscriber::EnvFilter;

fn main() -> Result<()> {
    // Set up tracing (to file or null to not interfere with TUI)
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    // Open database
    let db_path = {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        format!("{}/.0ai.db", home)
    };

    let db = Database::open(&db_path)?;

    // Build tokio runtime
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    runtime.block_on(async_main(db))
}

async fn async_main(db: Database) -> Result<()> {
    let mut app = App::new(db);

    // Startup: load config, prepare servers
    app.startup();

    // Start A2A server now that we're in async context
    app.start_a2a_server().await;

    // Create a default ephemeral session
    let ephemeral_id = uuid::Uuid::new_v4().to_string();
    let session = db::Session {
        id: ephemeral_id.clone(),
        name: None,
        model_id: app.active_model_id.clone(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        ephemeral: true,
    };
    let _ = app.db.save_session(&session);
    app.current_session_id = Some(ephemeral_id);

    // Run TUI (this blocks until user quits)
    ui::run(&mut app)?;

    Ok(())
}

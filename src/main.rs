use std::{env, net::SocketAddr, sync::Arc};

use codex_activity_compiler::{http, store::Database};

#[tokio::main]
async fn main() {
    let database_path =
        env::var("CODEX_ACTIVITY_DB").unwrap_or_else(|_| "codex-activity.sqlite".to_owned());
    let listen_addr: SocketAddr = env::var("CODEX_ACTIVITY_LISTEN")
        .unwrap_or_else(|_| "127.0.0.1:4318".to_owned())
        .parse()
        .expect("CODEX_ACTIVITY_LISTEN must be a socket address");
    let database = Arc::new(Database::open(&database_path).expect("open SQLite database"));
    let listener = tokio::net::TcpListener::bind(listen_addr)
        .await
        .expect("bind OTLP HTTP listener");

    eprintln!("OTLP/HTTP traces receiver listening on http://{listen_addr}/v1/traces");
    axum::serve(listener, http::router(database))
        .await
        .expect("serve OTLP HTTP receiver");
}

use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use warp::ws::Message;
use warp::Filter;

pub struct WsBroadcaster {
    tx: broadcast::Sender<String>,
    last_state: Arc<RwLock<String>>,
}

impl WsBroadcaster {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(64);
        Self {
            tx,
            last_state: Arc::new(RwLock::new(String::new())),
        }
    }

    pub fn send(&self, json: &str) {
        let _ = self.tx.send(json.to_string());
    }

    pub async fn set_welcome(&self, json: &str) {
        *self.last_state.write().await = json.to_string();
    }
}

pub async fn start_server(port: u16, broadcaster: Arc<WsBroadcaster>) {
    let bc = broadcaster.clone();
    let ws_route = warp::path("ws")
        .and(warp::ws())
        .map(move |ws: warp::ws::Ws| {
            let bc = bc.clone();
            ws.on_upgrade(move |socket| handle_connection(socket, bc))
        });

    let health = warp::path("health").map(|| {
        warp::reply::json(&serde_json::json!({"status": "ok", "version": "0.1.0"}))
    });

    let pill = warp::path!("overlay" / "pill")
        .map(|| warp::reply::html(include_str!("overlays/pill.html")));
    let glass = warp::path!("overlay" / "glass")
        .map(|| warp::reply::html(include_str!("overlays/glass.html")));
    let neon = warp::path!("overlay" / "neon")
        .map(|| warp::reply::html(include_str!("overlays/neon.html")));
    let widget = warp::path!("overlay" / "widget")
        .map(|| warp::reply::html(include_str!("overlays/widget.html")));

    let routes = ws_route
        .or(health)
        .or(pill)
        .or(glass)
        .or(neon)
        .or(widget);

    warp::serve(routes).run(([127, 0, 0, 1], port)).await;
}

async fn handle_connection(ws: warp::ws::WebSocket, bc: Arc<WsBroadcaster>) {
    let (mut tx, mut rx) = ws.split();
    let mut broadcast_rx = bc.tx.subscribe();

    let welcome = bc.last_state.read().await.clone();
    if !welcome.is_empty() {
        let _ = tx.send(Message::text(&welcome)).await;
    }

    let send_task = tokio::spawn(async move {
        while let Ok(msg) = broadcast_rx.recv().await {
            if tx.send(Message::text(&msg)).await.is_err() {
                break;
            }
        }
    });

    while rx.next().await.is_some() {}
    send_task.abort();
}

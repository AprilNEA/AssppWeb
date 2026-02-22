use axum::{
  extract::ws::{Message, WebSocket, WebSocketUpgrade},
  response::Response,
};
use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;

use asspp_core::wisp::{
  self, CloseReason, ConnectPayload, WispPacketType,
};

pub async fn wisp_handler(ws: WebSocketUpgrade) -> Response {
  ws.on_upgrade(handle_wisp)
}

async fn handle_wisp(socket: WebSocket) {
  let (mut ws_tx, mut ws_rx) = socket.split();

  // Channel for sending messages back through the WebSocket
  let (send_tx, mut send_rx) = mpsc::channel::<Vec<u8>>(256);

  // Active TCP streams keyed by stream_id
  let streams: std::sync::Arc<tokio::sync::Mutex<HashMap<u32, mpsc::Sender<Vec<u8>>>>> =
    std::sync::Arc::new(tokio::sync::Mutex::new(HashMap::new()));

  // WebSocket sender task
  let sender = tokio::spawn(async move {
    while let Some(data) = send_rx.recv().await {
      if ws_tx.send(Message::Binary(data.into())).await.is_err() {
        break;
      }
    }
  });

  // Process incoming WebSocket messages
  while let Some(msg) = ws_rx.next().await {
    let msg = match msg {
      Ok(Message::Binary(data)) => data.to_vec(),
      Ok(Message::Close(_)) | Err(_) => break,
      _ => continue,
    };

    let (ptype, stream_id, payload) = match wisp::parse_packet(&msg) {
      Some(p) => p,
      None => continue,
    };

    match ptype {
      WispPacketType::Connect => {
        let conn = match wisp::parse_connect(payload) {
          Some(c) => c,
          None => {
            let _ = send_tx
              .send(wisp::make_close_packet(stream_id, CloseReason::InvalidData))
              .await;
            continue;
          }
        };

        // Validate target
        if let Err(_) = wisp::validate_wisp_target(&conn.hostname, conn.port) {
          let _ = send_tx
            .send(wisp::make_close_packet(stream_id, CloseReason::Forbidden))
            .await;
          continue;
        }

        // Send initial CONTINUE
        let _ = send_tx
          .send(wisp::make_continue_packet(stream_id, 128))
          .await;

        // Spawn TCP connection
        let send_tx2 = send_tx.clone();
        let streams2 = streams.clone();
        let (tcp_tx, tcp_rx) = mpsc::channel::<Vec<u8>>(128);
        streams.lock().await.insert(stream_id, tcp_tx);

        tokio::spawn(handle_tcp_stream(
          stream_id,
          conn,
          send_tx2,
          tcp_rx,
          streams2,
        ));
      }

      WispPacketType::Data => {
        let streams_lock = streams.lock().await;
        if let Some(tcp_tx) = streams_lock.get(&stream_id) {
          let _ = tcp_tx.send(payload.to_vec()).await;
        }
      }

      WispPacketType::Close => {
        let mut streams_lock = streams.lock().await;
        streams_lock.remove(&stream_id);
      }

      WispPacketType::Continue => {
        // Client flow control — currently no-op
      }
    }
  }

  // Clean up all streams
  streams.lock().await.clear();
  sender.abort();
}

async fn handle_tcp_stream(
  stream_id: u32,
  conn: ConnectPayload,
  ws_send: mpsc::Sender<Vec<u8>>,
  mut tcp_rx: mpsc::Receiver<Vec<u8>>,
  streams: std::sync::Arc<tokio::sync::Mutex<HashMap<u32, mpsc::Sender<Vec<u8>>>>>,
) {
  let addr = format!("{}:{}", conn.hostname, conn.port);

  let tcp = match TcpStream::connect(&addr).await {
    Ok(s) => s,
    Err(_) => {
      let _ = ws_send
        .send(wisp::make_close_packet(stream_id, CloseReason::ServerRefused))
        .await;
      streams.lock().await.remove(&stream_id);
      return;
    }
  };

  let (mut read_half, mut write_half) = tcp.into_split();

  // TCP → WebSocket
  let ws_send2 = ws_send.clone();
  let read_task = tokio::spawn(async move {
    let mut buf = [0u8; 16384];
    loop {
      match read_half.read(&mut buf).await {
        Ok(0) => break,
        Ok(n) => {
          let packet = wisp::make_data_packet(stream_id, &buf[..n]);
          if ws_send2.send(packet).await.is_err() {
            break;
          }
        }
        Err(_) => break,
      }
    }
    let _ = ws_send2
      .send(wisp::make_close_packet(stream_id, CloseReason::Voluntary))
      .await;
  });

  // WebSocket → TCP
  let write_task = tokio::spawn(async move {
    while let Some(data) = tcp_rx.recv().await {
      if write_half.write_all(&data).await.is_err() {
        break;
      }
    }
  });

  // Wait for either direction to finish
  tokio::select! {
    _ = read_task => {},
    _ = write_task => {},
  }

  streams.lock().await.remove(&stream_id);
}

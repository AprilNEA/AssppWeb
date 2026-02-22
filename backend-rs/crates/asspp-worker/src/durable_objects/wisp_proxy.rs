use std::cell::RefCell;
use std::collections::HashMap;

use asspp_core::wisp::{self, CloseReason, WispPacketType};
use js_sys::{Boolean as JsBoolean, JsString, Number as JsNumber, Object as JsObject, Reflect, Uint8Array};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{ReadableStream, ReadableStreamDefaultReader, WritableStream, WritableStreamDefaultWriter};
use worker::*;

/// Per-stream state: the writable side of a TCP connection.
struct TcpStream {
  writer: WritableStreamDefaultWriter,
  #[allow(dead_code)]
  writable: WritableStream,
}

/// Durable Object that handles Wisp WebSocket connections.
/// Each connection gets its own DO instance.
/// Uses the Workers connect() API for TCP relay.
#[durable_object]
pub struct WispProxy {
  #[allow(dead_code)]
  state: State,
  #[allow(dead_code)]
  env: Env,
  /// Map of stream_id -> writable side of TCP connection
  streams: RefCell<HashMap<u32, TcpStream>>,
}

impl DurableObject for WispProxy {
  fn new(state: State, env: Env) -> Self {
    Self {
      state,
      env,
      streams: RefCell::new(HashMap::new()),
    }
  }

  async fn fetch(&self, _req: Request) -> Result<Response> {
    // Accept the WebSocket upgrade using the Hibernation API
    // so that websocket_message/websocket_close handlers fire.
    let WebSocketPair { client, server } = WebSocketPair::new()?;
    self.state.accept_web_socket(&server);

    // Send initial CONTINUE for stream 0 (protocol handshake)
    let continue_packet = wisp::make_continue_packet(0, 128);
    server.send_with_bytes(&continue_packet)?;

    Response::from_websocket(client)
  }

  async fn websocket_message(
    &self,
    ws: WebSocket,
    message: WebSocketIncomingMessage,
  ) -> Result<()> {
    let data = match message {
      WebSocketIncomingMessage::Binary(bytes) => bytes,
      WebSocketIncomingMessage::String(_) => return Ok(()),
    };

    let (ptype, stream_id, payload) = match wisp::parse_packet(&data) {
      Some(p) => p,
      None => return Ok(()),
    };

    match ptype {
      WispPacketType::Connect => {
        self.handle_connect(&ws, stream_id, payload);
      }

      WispPacketType::Data => {
        self.handle_data(stream_id, payload).await;
      }

      WispPacketType::Close => {
        self.handle_close(stream_id);
      }

      WispPacketType::Continue => {
        // Client flow control — no-op for now
      }
    }

    Ok(())
  }

  async fn websocket_close(
    &self,
    _ws: WebSocket,
    _code: usize,
    _reason: String,
    _was_clean: bool,
  ) -> Result<()> {
    // Clean up all TCP connections
    let mut streams = self.streams.borrow_mut();
    for (_, tcp) in streams.drain() {
      let _ = tcp.writer.close();
    }
    Ok(())
  }
}

impl WispProxy {
  /// Handle a CONNECT packet: validate target, open TCP socket, start read relay.
  fn handle_connect(&self, ws: &WebSocket, stream_id: u32, payload: &[u8]) {
    let conn = match wisp::parse_connect(payload) {
      Some(c) => c,
      None => {
        let _ = ws.send_with_bytes(&wisp::make_close_packet(
          stream_id,
          CloseReason::InvalidData,
        ));
        return;
      }
    };

    // Validate target
    if wisp::validate_wisp_target(&conn.hostname, conn.port).is_err() {
      let _ = ws.send_with_bytes(&wisp::make_close_packet(
        stream_id,
        CloseReason::Forbidden,
      ));
      return;
    }

    // Create TCP connection using worker_sys::connect() directly
    // to get independent readable/writable streams.
    let address = make_js_address(&conn.hostname, conn.port);
    let options = make_js_options();

    let raw_socket = match worker_sys::connect(address, options) {
      Ok(s) => s,
      Err(_e) => {
        let _ = ws.send_with_bytes(&wisp::make_close_packet(
          stream_id,
          CloseReason::ServerRefused,
        ));
        return;
      }
    };

    // Get separate readable/writable streams from the JS socket
    let readable: ReadableStream = match raw_socket.readable() {
      Ok(r) => r,
      Err(_) => {
        let _ = ws.send_with_bytes(&wisp::make_close_packet(
          stream_id,
          CloseReason::ServerRefused,
        ));
        return;
      }
    };

    let writable: WritableStream = match raw_socket.writable() {
      Ok(w) => w,
      Err(_) => {
        let _ = ws.send_with_bytes(&wisp::make_close_packet(
          stream_id,
          CloseReason::ServerRefused,
        ));
        return;
      }
    };

    // Get a writer for the writable stream (used for WS → TCP)
    let writer: WritableStreamDefaultWriter = match writable.get_writer() {
      Ok(w) => w.unchecked_into(),
      Err(_) => {
        let _ = ws.send_with_bytes(&wisp::make_close_packet(
          stream_id,
          CloseReason::ServerRefused,
        ));
        return;
      }
    };

    // Store the write side for DATA forwarding
    self.streams.borrow_mut().insert(stream_id, TcpStream {
      writer,
      writable,
    });

    // Send CONTINUE to client
    let _ = ws.send_with_bytes(&wisp::make_continue_packet(stream_id, 128));

    // Spawn a read relay task: TCP readable → Wisp DATA packets → WebSocket
    let ws_clone = ws.clone();
    wasm_bindgen_futures::spawn_local(async move {
      read_relay(readable, ws_clone, stream_id).await;
    });
  }

  /// Handle a DATA packet: forward payload to the TCP writable stream.
  async fn handle_data(&self, stream_id: u32, payload: &[u8]) {
    // Get a clone of the writer JS reference without holding the RefCell borrow across await
    let promise = {
      let streams = self.streams.borrow();
      match streams.get(&stream_id) {
        Some(tcp) => {
          let chunk = Uint8Array::from(payload);
          // write_with_chunk returns a Promise; don't release_lock since
          // the writer is reused for subsequent DATA packets on this stream
          Some(tcp.writer.write_with_chunk(&chunk.into()))
        }
        None => None,
      }
    };

    if let Some(promise) = promise {
      let _ = JsFuture::from(promise).await;
    }
  }

  /// Handle a CLOSE packet: close the TCP connection for this stream.
  fn handle_close(&self, stream_id: u32) {
    if let Some(tcp) = self.streams.borrow_mut().remove(&stream_id) {
      tcp.writer.release_lock();
      let _ = tcp.writable.close();
    }
  }
}

/// Read loop: reads from TCP ReadableStream, wraps in Wisp DATA packets, sends to WS.
async fn read_relay(readable: ReadableStream, ws: WebSocket, stream_id: u32) {
  let reader: ReadableStreamDefaultReader = match readable.get_reader().dyn_into() {
    Ok(r) => r,
    Err(_) => {
      let _ = ws.send_with_bytes(&wisp::make_close_packet(
        stream_id,
        CloseReason::NetworkError,
      ));
      return;
    }
  };

  loop {
    let result = match JsFuture::from(reader.read()).await {
      Ok(val) => val,
      Err(_) => {
        let _ = ws.send_with_bytes(&wisp::make_close_packet(
          stream_id,
          CloseReason::NetworkError,
        ));
        break;
      }
    };

    // Check { done, value } from the reader
    let done = Reflect::get(&result, &JsValue::from_str("done"))
      .unwrap_or(JsValue::TRUE);
    if done.is_truthy() {
      // TCP stream ended
      let _ = ws.send_with_bytes(&wisp::make_close_packet(
        stream_id,
        CloseReason::Voluntary,
      ));
      break;
    }

    let value = match Reflect::get(&result, &JsValue::from_str("value")) {
      Ok(v) => v,
      Err(_) => break,
    };

    let arr: Uint8Array = value.unchecked_into();
    let bytes = arr.to_vec();

    if bytes.is_empty() {
      continue;
    }

    // Wrap in Wisp DATA packet and send to WebSocket
    let packet = wisp::make_data_packet(stream_id, &bytes);
    if ws.send_with_bytes(&packet).is_err() {
      break;
    }
  }

  reader.release_lock();
}

/// Build a JS address object { hostname, port } for worker_sys::connect().
fn make_js_address(hostname: &str, port: u16) -> JsValue {
  let obj = JsObject::new();
  let _ = Reflect::set(&obj, &JsValue::from("hostname"), &JsString::from(hostname).into());
  let _ = Reflect::set(&obj, &JsValue::from("port"), &JsNumber::from(port as f64).into());
  obj.into()
}

/// Build JS socket options. secureTransport is "off" because TLS is handled
/// end-to-end by the client (libcurl.js WASM + Mbed TLS 1.3); the Wisp tunnel
/// must relay raw TCP bytes without adding another TLS layer.
fn make_js_options() -> JsValue {
  let obj = JsObject::new();
  let _ = Reflect::set(
    &obj,
    &JsValue::from("secureTransport"),
    &JsString::from("off").into(),
  );
  let _ = Reflect::set(
    &obj,
    &JsValue::from("allowHalfOpen"),
    &JsBoolean::from(false).into(),
  );
  obj.into()
}

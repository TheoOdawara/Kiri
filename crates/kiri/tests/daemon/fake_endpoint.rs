use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use crate::support::wait_until;

#[derive(Clone)]
pub enum Part {
    Text(&'static str),
    Delta(Value),
    Wait(Duration),
    Hold,
    BreakStream,
}

pub const FINISHED: [&str; 2] = [
    r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
    "[DONE]",
];

#[derive(Default)]
struct State {
    request_lines: Vec<String>,
    authorizations: Vec<String>,
    bodies: Vec<Value>,
    releases: usize,
    holding: usize,
    closed: usize,
}

pub struct FakeEndpoint {
    pub base_url: String,
    state: Arc<Mutex<State>>,
}

impl FakeEndpoint {
    pub fn answering(script: Vec<Part>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(State::default()));
        let shared = Arc::clone(&state);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let state = Arc::clone(&shared);
                let script = script.clone();
                std::thread::spawn(move || answer(stream, &script, &state));
            }
        });
        Self { base_url, state }
    }

    pub fn body(&self, request: usize) -> Value {
        wait_until(|| self.state.lock().unwrap().bodies.len() > request);
        self.state.lock().unwrap().bodies[request].clone()
    }

    pub fn request_line(&self, request: usize) -> String {
        self.body(request);
        self.state.lock().unwrap().request_lines[request].clone()
    }

    pub fn authorization(&self, request: usize) -> String {
        self.body(request);
        self.state.lock().unwrap().authorizations[request].clone()
    }

    pub fn wait_for_holding(&self, requests: usize) {
        wait_until(|| self.state.lock().unwrap().holding >= requests);
    }

    pub fn release(&self) {
        self.state.lock().unwrap().releases += 1;
    }

    pub fn wait_for_closed_connections(&self, requests: usize) {
        wait_until(|| self.state.lock().unwrap().closed >= requests);
    }
}

fn answer(stream: TcpStream, script: &[Part], state: &Mutex<State>) {
    let mut writer = stream.try_clone().unwrap();
    let mut reader = BufReader::new(stream);
    let mut request_line = String::new();
    reader.read_line(&mut request_line).unwrap();
    let mut content_length = 0;
    let mut authorization = String::new();
    loop {
        let mut header = String::new();
        reader.read_line(&mut header).unwrap();
        if header.trim().is_empty() {
            break;
        }
        let Some((name, value)) = header.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("content-length") {
            content_length = value.trim().parse().unwrap();
        } else if name.eq_ignore_ascii_case("authorization") {
            authorization = value.trim().to_owned();
        }
    }
    let mut body = vec![0; content_length];
    reader.read_exact(&mut body).unwrap();
    {
        let mut locked = state.lock().unwrap();
        locked.request_lines.push(request_line.trim().to_owned());
        locked.authorizations.push(authorization);
        locked.bodies.push(serde_json::from_slice(&body).unwrap());
    }

    writer
        .write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
        )
        .unwrap();
    for part in script {
        match part {
            Part::Text(text) => {
                if write_delta(&mut writer, &json!({"content": text})).is_err() {
                    break;
                }
            }
            Part::Delta(delta) => {
                if write_delta(&mut writer, delta).is_err() {
                    break;
                }
            }
            Part::Wait(duration) => std::thread::sleep(*duration),
            Part::BreakStream => return,
            Part::Hold => {
                if !hold(&reader, state) {
                    state.lock().unwrap().closed += 1;
                    return;
                }
            }
        }
    }
    for event in FINISHED {
        let _ = write_event(&mut writer, event);
    }
}

fn write_delta(writer: &mut TcpStream, delta: &Value) -> std::io::Result<()> {
    let chunk = json!({"choices": [{"delta": delta, "finish_reason": null}]});
    write_event(writer, &chunk.to_string())
}

fn write_event(writer: &mut TcpStream, data: &str) -> std::io::Result<()> {
    writer.write_all(format!("data: {data}\n\n").as_bytes())?;
    writer.flush()
}

fn hold(reader: &BufReader<TcpStream>, state: &Mutex<State>) -> bool {
    let seen_releases = {
        let mut locked = state.lock().unwrap();
        locked.holding += 1;
        locked.releases
    };
    let stream = reader.get_ref();
    stream
        .set_read_timeout(Some(Duration::from_millis(10)))
        .unwrap();
    loop {
        if state.lock().unwrap().releases > seen_releases {
            return true;
        }
        match stream.peek(&mut [0]) {
            Err(cause) if matches!(cause.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            _ => return false,
        }
    }
}

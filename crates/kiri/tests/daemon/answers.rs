use std::time::Duration;

use serde_json::{Value, json};

use crate::fake_endpoint::{FakeEndpoint, Part};
use crate::support::{Client, RawClient, TestHome, absolute_path};

const NO_TOOLS_YET: &str = "Kiri has no tools yet, so this call did not run.\n";

fn text_prompt(session: &str, text: &str) -> Value {
    json!({"sessionId": session, "prompt": [{"type": "text", "text": text}]})
}

fn tool_call_start(id: &str, name: &str, arguments: &str) -> Part {
    Part::Delta(json!({"tool_calls": [{
        "index": 0,
        "id": id,
        "type": "function",
        "function": {"name": name, "arguments": arguments},
    }]}))
}

fn tool_call_arguments(arguments: &str) -> Part {
    Part::Delta(json!({"tool_calls": [{"index": 0, "function": {"arguments": arguments}}]}))
}

fn updates_until_answer(client: &mut RawClient, id: i64) -> (Vec<Value>, Value) {
    let mut updates = Vec::new();
    loop {
        let message = client
            .read()
            .expect("the connection closed before the answer");
        if message["id"] == id {
            return (updates, message);
        }
        updates.push(message["params"]["update"].clone());
    }
}

fn chunk_texts(updates: &[Value], kind: &str) -> Vec<String> {
    updates
        .iter()
        .filter(|update| update["sessionUpdate"] == kind)
        .map(|update| update["content"]["text"].as_str().unwrap().to_owned())
        .collect()
}

fn error_of(answer: &Value) -> (i64, &str) {
    (
        answer["error"]["code"].as_i64().unwrap(),
        answer["error"]["message"].as_str().unwrap(),
    )
}

#[test]
fn a_first_prompt_starts_the_daemon_and_streams_the_reply_in_pieces() {
    let home = TestHome::new();
    let endpoint =
        FakeEndpoint::answering(vec![Part::Text("first "), Part::Hold, Part::Text("second")]);
    let mut client = Client::start(home.kiri_on(&endpoint.base_url, &[]));

    client.read_until("> ");
    client.send_line("hello");
    client.read_until("first ");
    endpoint.wait_for_holding(1);
    endpoint.release();
    client.read_until("second\n> ");

    assert_eq!(home.started_daemons(), 1);
    assert_eq!(
        endpoint.request_line(0),
        "POST /v1/chat/completions HTTP/1.1"
    );
    assert_eq!(endpoint.authorization(0), "Bearer test-key");
    assert_eq!(
        endpoint.body(0),
        json!({
            "model": "test-model",
            "messages": [{"role": "user", "content": "hello"}],
            "stream": true,
        })
    );
    client.end();
    home.wait_for_daemon_exit();
}

#[test]
fn two_clients_share_a_daemon_and_stream_at_the_same_time() {
    let home = TestHome::new();
    let endpoint =
        FakeEndpoint::answering(vec![Part::Text("first "), Part::Hold, Part::Text("second")]);
    let mut clients = [
        Client::start(home.kiri_on(&endpoint.base_url, &[])),
        Client::start(home.kiri_on(&endpoint.base_url, &[])),
    ];

    for client in &mut clients {
        client.read_until("> ");
        client.send_line("hello");
    }
    for client in &mut clients {
        client.read_until("first ");
    }
    endpoint.wait_for_holding(2);
    endpoint.release();
    for client in &mut clients {
        client.read_until("second\n> ");
    }

    assert_eq!(home.started_daemons(), 1);
    for client in clients {
        client.end();
    }
    home.wait_for_daemon_exit();
}

#[test]
fn a_client_leaving_closes_its_request_and_the_last_one_ends_the_daemon() {
    let home = TestHome::new();
    let endpoint = FakeEndpoint::answering(vec![Part::Text("first "), Part::Hold]);
    let mut leaving = Client::start(home.kiri_on(&endpoint.base_url, &[]));
    let mut last = Client::start(home.kiri_on(&endpoint.base_url, &[]));
    leaving.read_until("> ");
    last.read_until("> ");

    leaving.send_line("hello");
    leaving.read_until("first ");
    endpoint.wait_for_holding(1);
    leaving.kill();
    endpoint.wait_for_closed_connections(1);

    last.send_line("hello");
    last.read_until("first ");
    endpoint.wait_for_holding(2);
    last.kill();
    endpoint.wait_for_closed_connections(2);
    home.wait_for_daemon_exit();
    assert!(home.log().contains("daemon exited"));
}

#[test]
fn a_later_prompt_carries_the_conversation() {
    let home = TestHome::new();
    let endpoint = FakeEndpoint::answering(vec![Part::Text("reply")]);
    let mut client = Client::start(home.kiri_on(&endpoint.base_url, &[]));
    client.read_until("> ");

    client.send_line("first prompt");
    client.read_until("reply\n> ");
    client.send_line("second prompt");
    client.read_until("reply\n> ");

    assert_eq!(
        endpoint.body(1)["messages"],
        json!([
            {"role": "user", "content": "first prompt"},
            {"role": "assistant", "content": "reply"},
            {"role": "user", "content": "second prompt"},
        ])
    );
    client.end();
    home.wait_for_daemon_exit();
}

#[test]
fn an_invalid_configuration_stops_the_client_with_the_cause() {
    let long_model = "m".repeat(257);
    let cases: [(&str, &str, &str); 4] = [
        (
            "KIRI_API_KEY",
            "",
            "Set KIRI_BASE_URL, KIRI_API_KEY, and KIRI_MODEL before opening Kiri. Missing: KIRI_API_KEY.",
        ),
        (
            "KIRI_BASE_URL",
            "ftp://example.com",
            "KIRI_BASE_URL must be an http or https URL.",
        ),
        (
            "KIRI_MODEL",
            &long_model,
            "KIRI_MODEL is longer than 256 bytes.",
        ),
        ("KIRI_THINKING", "auto", "KIRI_THINKING must be on or off."),
    ];
    for (name, value, message) in cases {
        let home = TestHome::new();

        let output = home.kiri(&[]).env(name, value).output().unwrap();

        assert_eq!(output.status.code(), Some(1), "{name}");
        assert_eq!(String::from_utf8_lossy(&output.stderr).trim_end(), message);
        home.wait_for_daemon_exit();
    }
}

#[test]
fn a_catalog_setting_is_sent_in_its_model_format_and_reasoning_replays_by_catalog() {
    let deepseek_off = json!({"type": "disabled"});
    let cases = [
        (
            "deepseek-v4-pro",
            ("KIRI_REASONING_EFFORT", "high"),
            "reasoning_effort",
            json!("high"),
            true,
        ),
        (
            "deepseek-flash",
            ("KIRI_THINKING", "off"),
            "thinking",
            deepseek_off,
            true,
        ),
        (
            "kimi-k3.1-pro",
            ("KIRI_REASONING_EFFORT", "max"),
            "reasoning_effort",
            json!("max"),
            true,
        ),
        (
            "glm-5.3",
            ("KIRI_REASONING_EFFORT", "low"),
            "reasoning_effort",
            json!("low"),
            true,
        ),
        (
            "glm-5.3-flash",
            ("KIRI_REASONING_EFFORT", "high"),
            "reasoning_effort",
            json!("high"),
            true,
        ),
        (
            "qwen3.8-max",
            ("KIRI_THINKING", "off"),
            "enable_thinking",
            json!(false),
            false,
        ),
        (
            "qwen3.8-flash",
            ("KIRI_THINKING", "on"),
            "enable_thinking",
            json!(true),
            false,
        ),
        (
            "gemini-4-argon",
            ("KIRI_REASONING_EFFORT", "medium"),
            "reasoning_effort",
            json!("medium"),
            false,
        ),
        (
            "gemini-3.8-flash",
            ("KIRI_REASONING_EFFORT", "low"),
            "reasoning_effort",
            json!("low"),
            false,
        ),
    ];
    for (model, (variable, value), key, sent, replays) in cases {
        let home = TestHome::new();
        let endpoint = FakeEndpoint::answering(vec![
            Part::Delta(json!({"reasoning": "first "})),
            Part::Text("reply"),
            Part::Delta(json!({"reasoning_content": "second"})),
        ]);
        let mut command = home.kiri_on(&endpoint.base_url, &[]);
        command.env("KIRI_MODEL", model).env(variable, value);
        let mut client = Client::start(command);
        client.read_until("> ");

        client.send_line("first prompt");
        client.read_until("> ");
        client.send_line("second prompt");
        client.read_until("> ");

        let body = endpoint.body(1);
        assert_eq!(body["model"], model);
        assert_eq!(body[key], sent, "{model}");
        let mut assistant = json!({"role": "assistant", "content": "reply"});
        if replays {
            assistant["reasoning_content"] = json!("first second");
        }
        assert_eq!(body["messages"][1], assistant, "{model}");
        client.end();
        home.wait_for_daemon_exit();
    }
}

#[test]
fn a_model_outside_the_catalog_replays_no_reasoning_and_takes_the_pass_through_unchanged() {
    let home = TestHome::new();
    let endpoint = FakeEndpoint::answering(vec![
        Part::Delta(json!({"reasoning_content": "thought"})),
        Part::Text("reply"),
    ]);
    let mut command = home.kiri_on(&endpoint.base_url, &[]);
    command.env(
        "KIRI_REQUEST_PARAMS",
        r#"{"chat_template_kwargs":{"enable_thinking":true},"temperature":0.2}"#,
    );
    let mut client = Client::start(command);
    client.read_until("> ");

    client.send_line("first prompt");
    client.read_until("reply\n> ");
    client.send_line("second prompt");
    client.read_until("reply\n> ");

    assert_eq!(
        endpoint.body(1),
        json!({
            "model": "test-model",
            "messages": [
                {"role": "user", "content": "first prompt"},
                {"role": "assistant", "content": "reply"},
                {"role": "user", "content": "second prompt"},
            ],
            "stream": true,
            "chat_template_kwargs": {"enable_thinking": true},
            "temperature": 0.2,
        })
    );
    client.end();
    home.wait_for_daemon_exit();
}

#[test]
fn the_pass_through_overrides_the_catalog() {
    let home = TestHome::new();
    let endpoint = FakeEndpoint::answering(vec![Part::Text("reply")]);
    let mut command = home.kiri_on(&endpoint.base_url, &[]);
    command
        .env("KIRI_MODEL", "deepseek-v4-pro")
        .env("KIRI_REASONING_EFFORT", "high")
        .env("KIRI_REQUEST_PARAMS", r#"{"reasoning_effort":"max"}"#);
    let mut client = Client::start(command);
    client.read_until("> ");

    client.send_line("hello");
    client.read_until("reply\n> ");

    assert_eq!(endpoint.body(0)["reasoning_effort"], "max");
    client.end();
    home.wait_for_daemon_exit();
}

#[test]
fn a_refused_setting_stops_the_client_with_its_message_and_no_request() {
    let cases: [(&[(&str, &str)], &str); 6] = [
        (
            &[
                ("KIRI_MODEL", "glm-5.3"),
                ("KIRI_REASONING_EFFORT", "medium"),
            ],
            "glm-5.3 accepts these KIRI_REASONING_EFFORT values: low, high, max. Got: medium.",
        ),
        (
            &[
                ("KIRI_MODEL", "qwen3.8-max"),
                ("KIRI_REASONING_EFFORT", "low"),
            ],
            "qwen3.8-max has no reasoning effort setting in Kiri. Unset KIRI_REASONING_EFFORT, or use KIRI_REQUEST_PARAMS.",
        ),
        (
            &[("KIRI_MODEL", "glm-5.3"), ("KIRI_THINKING", "on")],
            "glm-5.3 has no thinking switch in Kiri. Unset KIRI_THINKING, or use KIRI_REQUEST_PARAMS.",
        ),
        (
            &[("KIRI_THINKING", "on")],
            "test-model is not in Kiri's catalog, so KIRI_REASONING_EFFORT and KIRI_THINKING do not apply. Use KIRI_REQUEST_PARAMS.",
        ),
        (
            &[("KIRI_REQUEST_PARAMS", "[1,2]")],
            "KIRI_REQUEST_PARAMS must be a JSON object.",
        ),
        (
            &[("KIRI_REQUEST_PARAMS", r#"{"stream":false}"#)],
            "KIRI_REQUEST_PARAMS cannot set stream, because Kiri sets it.",
        ),
    ];
    for (variables, message) in cases {
        let home = TestHome::new();
        let endpoint = FakeEndpoint::answering(vec![Part::Text("reply")]);
        let mut command = home.kiri_on(&endpoint.base_url, &[]);
        command.envs(variables.iter().copied());

        let output = command.output().unwrap();

        assert_eq!(output.status.code(), Some(1), "{message}");
        assert_eq!(String::from_utf8_lossy(&output.stderr).trim_end(), message);
        home.wait_for_daemon_exit();
        assert_eq!(endpoint.request_count(), 0, "{message}");
    }
}

#[test]
fn the_log_never_holds_the_request_params() {
    let marker = "request-params-marker";
    let cases = [
        format!(r#"{{"note":"{marker}"}}"#),
        format!("[\"{marker}\"]"),
    ];
    for params in cases {
        let home = TestHome::new();
        let endpoint = FakeEndpoint::answering(vec![Part::Text("reply")]);
        let mut command = home.kiri_on(&endpoint.base_url, &[]);
        command
            .env("KIRI_REQUEST_PARAMS", &params)
            .stdin(std::process::Stdio::null());

        let output = command.output().unwrap();

        assert!(!String::from_utf8_lossy(&output.stderr).contains(marker));
        home.wait_for_daemon_exit();
        assert!(!home.log().is_empty());
        assert!(!home.log().contains(marker));
    }
}

#[test]
fn an_invalid_configuration_stops_listening_before_the_answer() {
    let home = TestHome::new();
    let mut daemon = home
        .kiri(&["daemon"])
        .env_remove("KIRI_API_KEY")
        .spawn()
        .unwrap();
    home.wait_until(|| home.started_daemons() > 0);
    let mut client = home.raw_client();

    let answer = client.call(
        1,
        "session/new",
        json!({"cwd": absolute_path(), "mcpServers": []}),
    );

    assert_eq!(answer["error"]["code"], -32603);
    assert!(home.open_endpoint().is_err());
    drop(client);
    assert_eq!(daemon.wait().unwrap().code(), Some(0));
}

#[test]
fn a_fix_takes_effect_at_the_next_open() {
    let home = TestHome::new();
    for _ in 0..5 {
        let refused = home.kiri(&[]).env_remove("KIRI_API_KEY").output().unwrap();
        assert_eq!(refused.status.code(), Some(1));

        let mut client = Client::start(home.kiri(&[]));
        client.read_until("> ");

        client.end();
        home.wait_for_daemon_exit();
    }
}

#[test]
fn protocol_misuse_gets_the_specified_answers() {
    let home = TestHome::new();
    let endpoint = FakeEndpoint::answering(vec![Part::Hold]);
    let mut daemon = home.start_daemon_on(&endpoint.base_url);
    let mut client = home.raw_client();
    let session = client.new_session(1);

    let relative_cwd = client.call(
        2,
        "session/new",
        json!({"cwd": "relative/path", "mcpServers": []}),
    );
    assert_eq!(
        error_of(&relative_cwd),
        (-32602, "cwd must be an absolute path.")
    );

    let blank = client.call(3, "session/prompt", text_prompt(&session, " \n "));
    assert_eq!(error_of(&blank), (-32602, "The prompt is empty."));

    let image = client.call(
        4,
        "session/prompt",
        json!({"sessionId": session, "prompt": [
            {"type": "image", "data": "", "mimeType": "image/png"},
        ]}),
    );
    assert_eq!(
        error_of(&image),
        (
            -32602,
            "Kiri accepts only text and resource links in a prompt."
        )
    );

    let unknown_method = client.call(5, "session/unknown", json!({}));
    assert_eq!(unknown_method["error"]["code"], -32601);

    client.request(6, "session/prompt", text_prompt(&session, "hello"));
    endpoint.wait_for_holding(1);
    let during_a_turn = client.call(7, "session/prompt", text_prompt(&session, "again"));
    assert_eq!(
        error_of(&during_a_turn),
        (-32602, "A turn is already running in this session.")
    );

    let mut other = home.raw_client();
    let foreign = other.call(1, "session/prompt", text_prompt(&session, "hello"));
    assert_eq!(
        error_of(&foreign),
        (-32002, format!("Unknown session: {session}.").as_str())
    );

    drop_after_an_oversized_line(other);
    let still_up = client.call(8, "session/unknown", json!({}));
    assert_eq!(still_up["error"]["code"], -32601);

    drop(client);
    assert_eq!(daemon.wait().unwrap().code(), Some(0));
}

fn drop_after_an_oversized_line(mut client: RawClient) {
    let _ = client.write(&vec![b'x'; 16 * 1024 * 1024 + 1]);
    assert_eq!(client.read(), None);
}

#[test]
fn a_resource_link_reaches_the_endpoint_as_its_uri() {
    let home = TestHome::new();
    let endpoint = FakeEndpoint::answering(vec![Part::Text("reply")]);
    let mut daemon = home.start_daemon_on(&endpoint.base_url);
    let mut client = home.raw_client();
    let session = client.new_session(1);

    let answer = client.call(
        2,
        "session/prompt",
        json!({"sessionId": session, "prompt": [
            {"type": "text", "text": "Read this"},
            {"type": "resource_link", "name": "notes", "uri": "file:///tmp/notes.md"},
        ]}),
    );

    assert_eq!(answer["result"]["stopReason"], "end_turn");
    assert_eq!(
        endpoint.body(0)["messages"],
        json!([{"role": "user", "content": "Read this\nfile:///tmp/notes.md"}])
    );
    drop(client);
    assert_eq!(daemon.wait().unwrap().code(), Some(0));
}

#[test]
fn reasoning_text_and_a_tool_call_show_in_arrival_order_before_the_turn_ends() {
    let home = TestHome::new();
    let endpoint = FakeEndpoint::answering(vec![
        Part::Delta(json!({"reasoning_content": "thinking"})),
        Part::Hold,
        Part::Text("answer"),
        Part::Hold,
        tool_call_start("call_a", "lookup", ""),
        Part::Hold,
    ]);
    let mut client = Client::start(home.kiri_on(&endpoint.base_url, &[]));

    client.read_until("> ");
    client.send_line("hello");
    assert_eq!(client.read_until("thinking"), "[reasoning] thinking");
    endpoint.wait_for_holding(1);
    endpoint.release();
    assert_eq!(client.read_until("answer"), "\nanswer");
    endpoint.wait_for_holding(2);
    endpoint.release();
    assert_eq!(
        client.read_until("[tool call] lookup\n"),
        "\n[tool call] lookup\n"
    );
    endpoint.wait_for_holding(3);
    endpoint.release();
    assert_eq!(
        client.read_until("> "),
        format!("[tool call] lookup\n{NO_TOOLS_YET}> ")
    );

    client.end();
    home.wait_for_daemon_exit();
}

#[test]
fn both_reasoning_fields_arrive_as_thought_chunks() {
    for field in ["reasoning_content", "reasoning"] {
        let home = TestHome::new();
        let endpoint = FakeEndpoint::answering(vec![
            Part::Delta(json!({ field: "thought" })),
            Part::Text("reply"),
        ]);
        let mut daemon = home.start_daemon_on(&endpoint.base_url);
        let mut client = home.raw_client();
        let session = client.new_session(1);

        client.request(2, "session/prompt", text_prompt(&session, "hello"));
        let (updates, _) = updates_until_answer(&mut client, 2);

        assert_eq!(
            chunk_texts(&updates, "agent_thought_chunk"),
            ["thought"],
            "{field}"
        );
        assert_eq!(
            chunk_texts(&updates, "agent_message_chunk"),
            ["reply"],
            "{field}"
        );
        drop(client);
        assert_eq!(daemon.wait().unwrap().code(), Some(0));
    }
}

#[test]
fn a_tool_call_in_fragments_shows_joined_and_stays_out_of_the_history() {
    let home = TestHome::new();
    let endpoint = FakeEndpoint::answering(vec![
        Part::Text("before"),
        tool_call_start("call_a", "lookup", "{\"pa"),
        tool_call_arguments("th\":\"a\"}"),
    ]);
    let mut client = Client::start(home.kiri_on(&endpoint.base_url, &[]));

    client.read_until("> ");
    client.send_line("first");
    assert_eq!(
        client.read_until(NO_TOOLS_YET),
        format!(
            "before\n[tool call] lookup\n[tool call] lookup {{\"path\":\"a\"}}\n{NO_TOOLS_YET}"
        )
    );
    client.read_until("> ");
    client.send_line("next");
    assert_eq!(
        endpoint.body(1)["messages"],
        json!([
            {"role": "user", "content": "first"},
            {"role": "assistant", "content": "before"},
            {"role": "user", "content": "next"},
        ])
    );
    client.read_until(NO_TOOLS_YET);

    client.end();
    home.wait_for_daemon_exit();
}

#[test]
fn a_reply_with_no_text_adds_nothing_to_the_conversation() {
    let home = TestHome::new();
    let endpoint = FakeEndpoint::answering(vec![
        Part::Delta(json!({"reasoning": "thought"})),
        tool_call_start("call_a", "lookup", "{}"),
    ]);
    let mut client = Client::start(home.kiri_on(&endpoint.base_url, &[]));

    client.read_until("> ");
    client.send_line("first");
    client.read_until(NO_TOOLS_YET);
    client.read_until("> ");
    client.send_line("next");
    assert_eq!(
        endpoint.body(1)["messages"],
        json!([{"role": "user", "content": "next"}])
    );
    client.read_until(NO_TOOLS_YET);

    client.end();
    home.wait_for_daemon_exit();
}

#[test]
fn batching_reduces_the_events_and_keeps_the_text() {
    let home = TestHome::new();
    let mut script = Vec::new();
    let mut expected = String::new();
    for number in 0..1000 {
        let text = number.to_string();
        expected.push_str(&text);
        script.push(Part::Delta(json!({"content": text})));
        script.push(Part::Wait(Duration::from_millis(1)));
    }
    let endpoint = FakeEndpoint::answering(script);
    let mut daemon = home.start_daemon_on(&endpoint.base_url);
    let mut client = home.raw_client();
    let session = client.new_session(1);

    client.request(2, "session/prompt", text_prompt(&session, "hello"));
    let (updates, _) = updates_until_answer(&mut client, 2);

    let chunks = chunk_texts(&updates, "agent_message_chunk");
    assert!(chunks.len() < 500, "{} events", chunks.len());
    assert_eq!(chunks.concat(), expected);
    drop(client);
    assert_eq!(daemon.wait().unwrap().code(), Some(0));
}

#[test]
fn a_cancel_sends_the_open_text_and_fails_the_open_tool_call_before_the_answer() {
    let home = TestHome::new();
    let endpoint = FakeEndpoint::answering(vec![
        tool_call_start("call_a", "lookup", "{}"),
        Part::Text("partial"),
        Part::Hold,
    ]);
    let mut daemon = home.start_daemon_on(&endpoint.base_url);
    let mut client = home.raw_client();
    let session = client.new_session(1);

    client.request(2, "session/prompt", text_prompt(&session, "hello"));
    endpoint.wait_for_holding(1);
    client.send(json!({
        "jsonrpc": "2.0",
        "method": "session/cancel",
        "params": {"sessionId": session},
    }));
    let (updates, answer) = updates_until_answer(&mut client, 2);

    assert_eq!(answer["result"]["stopReason"], "cancelled");
    assert_eq!(chunk_texts(&updates, "agent_message_chunk"), ["partial"]);
    let last = updates.last().unwrap();
    assert_eq!(last["sessionUpdate"], "tool_call_update");
    assert_eq!(last["toolCallId"], "call_a");
    assert_eq!(last["status"], "failed");
    assert_eq!(last["rawInput"], "{}");

    drop(client);
    assert_eq!(daemon.wait().unwrap().code(), Some(0));
}

#[test]
fn a_broken_stream_sends_the_open_text_and_fails_the_open_tool_call_before_the_error() {
    let home = TestHome::new();
    let endpoint = FakeEndpoint::answering(vec![
        tool_call_start("call_a", "lookup", "{}"),
        Part::Text("partial"),
        Part::BreakStream,
    ]);
    let mut daemon = home.start_daemon_on(&endpoint.base_url);
    let mut client = home.raw_client();
    let session = client.new_session(1);

    client.request(2, "session/prompt", text_prompt(&session, "hello"));
    let (updates, answer) = updates_until_answer(&mut client, 2);

    assert_eq!(error_of(&answer).0, -32603);
    assert_eq!(chunk_texts(&updates, "agent_message_chunk"), ["partial"]);
    let last = updates.last().unwrap();
    assert_eq!(last["sessionUpdate"], "tool_call_update");
    assert_eq!(last["status"], "failed");

    drop(client);
    assert_eq!(daemon.wait().unwrap().code(), Some(0));
}

#[test]
fn a_cancel_closes_the_request_and_leaves_the_conversation_unchanged() {
    let home = TestHome::new();
    let endpoint = FakeEndpoint::answering(vec![Part::Hold, Part::Text("reply")]);
    let mut daemon = home.start_daemon_on(&endpoint.base_url);
    let mut client = home.raw_client();
    let session = client.new_session(1);

    client.request(
        2,
        "session/prompt",
        text_prompt(&session, "cancelled prompt"),
    );
    endpoint.wait_for_holding(1);
    client.send(json!({
        "jsonrpc": "2.0",
        "method": "session/cancel",
        "params": {"sessionId": session},
    }));

    endpoint.wait_for_closed_connections(1);
    assert_eq!(
        client.read_answer_to(2)["result"]["stopReason"],
        "cancelled"
    );

    client.request(3, "session/prompt", text_prompt(&session, "next prompt"));
    endpoint.wait_for_holding(2);
    assert_eq!(
        endpoint.body(1)["messages"],
        json!([{"role": "user", "content": "next prompt"}])
    );
    endpoint.release();
    assert_eq!(client.read_answer_to(3)["result"]["stopReason"], "end_turn");

    drop(client);
    assert_eq!(daemon.wait().unwrap().code(), Some(0));
}

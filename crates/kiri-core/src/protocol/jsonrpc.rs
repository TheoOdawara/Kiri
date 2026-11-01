use agent_client_protocol_schema::ProtocolVersion;
use agent_client_protocol_schema::rpc::{JsonRpcMessage, Notification};
use agent_client_protocol_schema::v1::{
    Error, Implementation, InitializeRequest, InitializeResponse, Request, RequestId, Response,
};
use serde::Serialize;
use serde_json::Value;

pub(crate) enum Incoming {
    Request(Request<Value>),
    Notification {
        method: String,
        params: Option<Value>,
    },
    Malformed(Error),
}

pub(crate) fn decode(line: &[u8]) -> Incoming {
    let Ok(message) = serde_json::from_slice::<Value>(line) else {
        return Incoming::Malformed(Error::parse_error());
    };
    if let Some(object) = message.as_object()
        && !object.contains_key("id")
    {
        return Incoming::Notification {
            method: object
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            params: object.get("params").cloned(),
        };
    }
    match serde_json::from_value::<JsonRpcMessage<Request<Value>>>(message) {
        Ok(request) => Incoming::Request(request.into_inner()),
        Err(_) => Incoming::Malformed(Error::invalid_request()),
    }
}

pub(crate) fn initialize(params: Value) -> Result<Value, Error> {
    serde_json::from_value::<InitializeRequest>(params).map_err(|_| Error::invalid_params())?;
    let response = InitializeResponse::new(ProtocolVersion::V1)
        .agent_info(Implementation::new("kiri", env!("CARGO_PKG_VERSION")));
    serde_json::to_value(response).map_err(|_| Error::internal_error())
}

pub(crate) fn response_line(id: RequestId, result: Result<Value, Error>) -> Vec<u8> {
    line_of(&JsonRpcMessage::wrap(Response::new(id, result)))
}

pub(crate) fn notification_line<P: Serialize>(method: &str, params: P) -> Vec<u8> {
    line_of(&JsonRpcMessage::wrap(Notification {
        method: method.into(),
        params: Some(params),
    }))
}

pub(crate) fn line_of<M: Serialize>(message: &M) -> Vec<u8> {
    let mut line = serde_json::to_vec(message).expect("an ACP message always serializes");
    line.push(b'\n');
    line
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn initialize_answers_version_one_and_kiri() {
        let answer = initialize(json!({"protocolVersion": 99})).unwrap();

        assert_eq!(answer["protocolVersion"], 1);
        assert_eq!(answer["authMethods"], json!([]));
        assert_eq!(answer["agentInfo"]["name"], "kiri");
        assert_eq!(answer["agentInfo"]["version"], env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn initialize_without_a_version_is_invalid_params() {
        let error = initialize(json!({})).unwrap_err();
        assert_eq!(i32::from(error.code), -32602);
    }

    #[test]
    fn invalid_json_is_a_parse_error() {
        let Incoming::Malformed(error) = decode(b"{not json") else {
            panic!("the line was accepted");
        };
        assert_eq!(i32::from(error.code), -32700);
    }

    #[test]
    fn a_message_with_no_id_is_a_notification() {
        let Incoming::Notification { method, .. } =
            decode(br#"{"jsonrpc":"2.0","method":"session/cancel"}"#)
        else {
            panic!("not read as a notification");
        };
        assert_eq!(method, "session/cancel");
    }

    #[test]
    fn a_response_line_carries_the_request_id_and_ends_in_a_newline() {
        let line = response_line(RequestId::Number(7), Ok(json!({"stopReason": "end_turn"})));

        assert_eq!(line.last(), Some(&b'\n'));
        let answer: Value = serde_json::from_slice(&line).unwrap();
        assert_eq!(answer["jsonrpc"], "2.0");
        assert_eq!(answer["id"], 7);
        assert_eq!(answer["result"]["stopReason"], "end_turn");
    }
}

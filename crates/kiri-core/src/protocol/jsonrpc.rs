use agent_client_protocol_schema::ProtocolVersion;
use agent_client_protocol_schema::rpc::JsonRpcMessage;
use agent_client_protocol_schema::v1::{
    AGENT_METHOD_NAMES, Error, Implementation, InitializeRequest, InitializeResponse, Request,
    RequestId, Response,
};
use serde_json::Value;

pub(crate) fn answer(line: &[u8]) -> Option<Vec<u8>> {
    let message = match serde_json::from_slice::<Value>(line) {
        Ok(message) => message,
        Err(_) => return Some(encode(RequestId::Null, Err(Error::parse_error()))),
    };
    if message
        .as_object()
        .is_some_and(|object| !object.contains_key("id"))
    {
        return None;
    }
    let request = match serde_json::from_value::<JsonRpcMessage<Request<Value>>>(message) {
        Ok(request) => request.into_inner(),
        Err(_) => return Some(encode(RequestId::Null, Err(Error::invalid_request()))),
    };
    Some(encode(
        request.id,
        dispatch(&request.method, request.params),
    ))
}

fn dispatch(method: &str, params: Option<Value>) -> Result<Value, Error> {
    if method != AGENT_METHOD_NAMES.initialize {
        return Err(Error::method_not_found());
    }
    serde_json::from_value::<InitializeRequest>(params.unwrap_or_default())
        .map_err(|_| Error::invalid_params())?;
    let response = InitializeResponse::new(ProtocolVersion::V1)
        .agent_info(Implementation::new("kiri", env!("CARGO_PKG_VERSION")));
    serde_json::to_value(response).map_err(|_| Error::internal_error())
}

fn encode(id: RequestId, result: Result<Value, Error>) -> Vec<u8> {
    let response = JsonRpcMessage::wrap(Response::new(id, result));
    let mut line = serde_json::to_vec(&response).expect("a JSON value always serializes");
    line.push(b'\n');
    line
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn answer_to(message: &str) -> Value {
        serde_json::from_slice(&answer(message.as_bytes()).unwrap()).unwrap()
    }

    #[test]
    fn initialize_answers_version_one_and_kiri() {
        let answer = answer_to(
            r#"{"jsonrpc":"2.0","id":7,"method":"initialize","params":{"protocolVersion":99}}"#,
        );

        assert_eq!(answer["id"], 7);
        assert_eq!(answer["result"]["protocolVersion"], 1);
        assert_eq!(answer["result"]["authMethods"], json!([]));
        assert_eq!(answer["result"]["agentInfo"]["name"], "kiri");
        assert_eq!(
            answer["result"]["agentInfo"]["version"],
            env!("CARGO_PKG_VERSION")
        );
    }

    #[test]
    fn initialize_without_a_version_is_invalid_params() {
        let answer = answer_to(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#);
        assert_eq!(answer["error"]["code"], -32602);
    }

    #[test]
    fn an_unknown_method_is_not_found() {
        let answer = answer_to(r#"{"jsonrpc":"2.0","id":1,"method":"session/new"}"#);
        assert_eq!(answer["error"]["code"], -32601);
    }

    #[test]
    fn invalid_json_is_a_parse_error_with_a_null_id() {
        let answer = answer_to("{not json");
        assert_eq!(answer["error"]["code"], -32700);
        assert_eq!(answer["id"], Value::Null);
    }

    #[test]
    fn a_notification_gets_no_answer() {
        assert!(answer(br#"{"jsonrpc":"2.0","method":"session/cancel"}"#).is_none());
    }
}

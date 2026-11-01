mod endpoint;
pub(crate) mod framing;
pub(crate) mod jsonrpc;
pub(crate) mod transport;

use std::io;

use agent_client_protocol_schema::ProtocolVersion;
use agent_client_protocol_schema::rpc::JsonRpcMessage;
use agent_client_protocol_schema::v1::{
    AGENT_METHOD_NAMES, Implementation, InitializeRequest, InitializeResponse, Request, RequestId,
    Response,
};
use thiserror::Error;
use tokio::io::{AsyncWriteExt, BufReader, ReadHalf, WriteHalf};

pub use endpoint::{Endpoint, EndpointError};

#[derive(Debug, Error)]
pub enum ConnectError {
    #[error("The Kiri daemon pipe belongs to another user. Refusing to connect.")]
    ForeignPipe,
    #[error(transparent)]
    Io(#[from] io::Error),
}

pub struct Connection {
    reader: BufReader<ReadHalf<transport::ClientStream>>,
    writer: WriteHalf<transport::ClientStream>,
}

pub async fn connect(endpoint: &Endpoint) -> Result<Connection, ConnectError> {
    let stream = transport::connect(endpoint).await?;
    let (reader, writer) = tokio::io::split(stream);
    let mut connection = Connection {
        reader: BufReader::new(reader),
        writer,
    };
    connection.initialize().await?;
    Ok(connection)
}

impl Connection {
    async fn initialize(&mut self) -> io::Result<()> {
        let request = JsonRpcMessage::wrap(Request {
            id: RequestId::Number(1),
            method: AGENT_METHOD_NAMES.initialize.into(),
            params: Some(
                InitializeRequest::new(ProtocolVersion::V1)
                    .client_info(Implementation::new("kiri", env!("CARGO_PKG_VERSION"))),
            ),
        });
        let mut line = serde_json::to_vec(&request)?;
        line.push(b'\n');
        self.writer.write_all(&line).await?;

        let answer = framing::read_line(&mut self.reader)
            .await?
            .ok_or_else(|| io::Error::from(io::ErrorKind::UnexpectedEof))?;
        let answer: JsonRpcMessage<Response<InitializeResponse>> = serde_json::from_slice(&answer)?;
        match answer.into_inner() {
            Response::Result { .. } => Ok(()),
            Response::Error { error, .. } => Err(io::Error::other(error.message)),
        }
    }

    pub async fn closed(&mut self) {
        while let Ok(Some(_)) = framing::read_line(&mut self.reader).await {}
    }
}

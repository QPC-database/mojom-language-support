use std::io::{self, Write};

use serde::{Deserialize, Serialize};
use serde_json::{from_slice, from_value, Value};

#[derive(Debug)]
pub(crate) enum Error {
    ProtocolError(String),
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Error {
        Error::ProtocolError(err.to_string())
    }
}

pub(crate) type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Message {
    Request(RequestMessage),
    Response(ResponseMessage),
    Notofication(NotificationMessage),
}

impl Message {
    fn from_slice(buf: &[u8]) -> serde_json::Result<Message> {
        from_slice::<Message>(buf)
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RequestMessage {
    pub id: u64,
    pub method: String,
    pub params: Value,
}

pub(crate) fn into_request_id_params<R>(req: RequestMessage) -> Result<(u64, R::Params)>
where
    R: lsp_types::request::Request,
    R::Params: serde::de::DeserializeOwned,
{
    if req.method != R::METHOD {
        let error_message = format!("Expected {} but got {}", R::METHOD, req.method);
        return Err(Error::ProtocolError(error_message));
    }
    let params = from_value(req.params).map_err(|err| {
        let error_message = format!("Failed to parse {} message.\n{:?}", R::METHOD, err);
        Error::ProtocolError(error_message)
    })?;
    Ok((req.id, params))
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ResponseMessage {
    pub id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ResponseError>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ResponseError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl ResponseError {
    pub fn new(code: ErrorCodes, message: String) -> ResponseError {
        ResponseError {
            code: code.into(),
            message: message,
            data: None,
        }
    }
}

pub enum ErrorCodes {
    // Defined by JSON RPC
    ParseError,
    InvalidRequest,
    MethodNotFound,
    InvalidParams,
    InternalError,
    #[allow(non_camel_case_types)]
    serverErrorStart,
    #[allow(non_camel_case_types)]
    serverErrorEnd,
    ServerNotInitialized,
    UnknownErrorCode,

    // Defined by the protocol
    RequestCancelled,
    ContentModified,
}

impl From<ErrorCodes> for i32 {
    fn from(code: ErrorCodes) -> i32 {
        match code {
            ErrorCodes::ParseError => -32700,
            ErrorCodes::InvalidRequest => -32600,
            ErrorCodes::MethodNotFound => -32601,
            ErrorCodes::InvalidParams => -32602,
            ErrorCodes::InternalError => -32603,
            ErrorCodes::serverErrorStart => -32099,
            ErrorCodes::serverErrorEnd => -32000,
            ErrorCodes::ServerNotInitialized => -32002,
            ErrorCodes::UnknownErrorCode => -32001,
            ErrorCodes::RequestCancelled => -32800,
            ErrorCodes::ContentModified => -32801,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NotificationMessage {
    pub method: String,
    pub params: Value,
}

pub(crate) fn into_notification_params<N>(notif: NotificationMessage) -> Result<N::Params>
where
    N: lsp_types::notification::Notification,
    N::Params: serde::de::DeserializeOwned,
{
    if notif.method != N::METHOD {
        let error_message = format!("Expected {} but got {}", N::METHOD, notif.method);
        return Err(Error::ProtocolError(error_message));
    }
    let params = from_value(notif.params).map_err(|err| {
        let error_message = format!("Failed to parse {} message.\n{:?}", N::METHOD, err);
        Error::ProtocolError(error_message)
    })?;
    Ok(params)
}

// https://microsoft.github.io/language-server-protocol/specification#header-part
struct Header {
    pub content_length: usize,
}

fn read_header(reader: &mut impl io::BufRead) -> io::Result<Header> {
    let mut content_length = None;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "No header"));
        }
        if line == "\r\n" {
            break;
        }

        let header_fields = line.trim().split(": ").collect::<Vec<_>>();
        if header_fields.len() != 2 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid header",
            ));
        }
        let name = header_fields[0].to_ascii_lowercase();
        let value = header_fields[1];

        if name == "content-length" {
            let value = match value.parse::<usize>() {
                Ok(n) => n,
                Err(e) => return Err(io::Error::new(io::ErrorKind::InvalidInput, e)),
            };
            content_length = Some(value);
        }
    }

    content_length
        .map(|n| Header { content_length: n })
        .ok_or(io::Error::new(
            io::ErrorKind::InvalidInput,
            "No content type",
        ))
}

pub(crate) fn read_message(reader: &mut impl io::BufRead) -> Result<Message> {
    let header = read_header(reader)?;
    let mut buf = vec![0; header.content_length];
    reader.read_exact(&mut buf)?;
    match Message::from_slice(&buf) {
        Ok(message) => Ok(message),
        Err(_) => Err(Error::ProtocolError("Failed to parse message".to_owned())),
    }
}

#[derive(Serialize)]
struct JsonRpcResponseMessage<'a> {
    jsonrpc: &'a str,
    id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ResponseError>,
}

fn write_message<M: Serialize>(writer: &mut impl Write, message: M) -> Result<()> {
    let message = serde_json::to_string(&message).map_err(|err| {
        let error_message = err.to_string();
        Error::ProtocolError(error_message)
    })?;

    let content_length = message.len();

    write!(writer, "Content-Length: {}\r\n\r\n", content_length)?;
    writer.write_all(message.as_bytes())?;
    writer.flush()?;
    Ok(())
}

pub(crate) fn write_success_result<R>(writer: &mut impl Write, id: u64, res: R) -> Result<()>
where
    R: serde::Serialize,
{
    let res = serde_json::to_value(&res).map_err(|err| {
        let error_message = err.to_string();
        Error::ProtocolError(error_message)
    })?;
    write_success_response(writer, id, res)
}

pub(crate) fn write_success_response(
    writer: &mut impl Write,
    id: u64,
    result: Value,
) -> Result<()> {
    let message = JsonRpcResponseMessage {
        jsonrpc: "2.0",
        id: id,
        result: Some(result),
        error: None,
    };
    write_message(writer, message)
}

pub(crate) fn write_error_response(
    writer: &mut impl Write,
    id: u64,
    error: ResponseError,
) -> Result<()> {
    let message = JsonRpcResponseMessage {
        jsonrpc: "2.0",
        id: id,
        result: None,
        error: Some(error),
    };
    write_message(writer, message)
}

#[derive(Serialize)]
struct JsonRpcNotificationMessage<'a> {
    jsonrpc: &'a str,
    method: &'a str,
    params: Value,
}

pub(crate) fn write_notification(
    writer: &mut impl Write,
    method: &str,
    params: Value,
) -> Result<()> {
    let message = JsonRpcNotificationMessage {
        jsonrpc: "2.0",
        method: method,
        params: params,
    };
    write_message(writer, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_header() {
        let input = b"content-length: 208\r\n\r\n";
        let mut reader = io::BufReader::new(&input[..]);
        let header = read_header(&mut reader).unwrap();
        assert_eq!(208, header.content_length);
    }
}

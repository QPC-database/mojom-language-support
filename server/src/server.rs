use std::io::{self, BufRead, BufReader, BufWriter, Write};

use serde_json::Value;

use crate::protocol::{
    read_message, write_error_response, write_success_response, write_success_result, ErrorCodes,
    Message, NotificationMessage, RequestMessage, ResponseError,
};

use crate::{Error, Result};

#[derive(PartialEq)]
enum State {
    Initialized,
    ShuttingDown,
}

struct ServerContext {
    state: State,
    // Set when `exit` notification is received.
    exit_code: Option<i32>,
}

impl ServerContext {
    fn new() -> ServerContext {
        ServerContext {
            state: State::Initialized,
            exit_code: None,
        }
    }
}

fn create_server_capabilities() -> lsp_types::ServerCapabilities {
    let options = lsp_types::TextDocumentSyncOptions {
        open_close: Some(true),
        change: Some(lsp_types::TextDocumentSyncKind::Full),
        will_save: Some(true),
        will_save_wait_until: Some(false),
        save: None,
    };

    let text_document_sync = lsp_types::TextDocumentSyncCapability::Options(options);

    lsp_types::ServerCapabilities {
        text_document_sync: Some(text_document_sync),
        hover_provider: None,
        completion_provider: None,
        signature_help_provider: None,
        definition_provider: None,
        type_definition_provider: None,
        implementation_provider: None,
        references_provider: None,
        document_highlight_provider: None,
        document_symbol_provider: None,
        workspace_symbol_provider: None,
        code_action_provider: None,
        code_lens_provider: None,
        document_formatting_provider: None,
        document_range_formatting_provider: None,
        document_on_type_formatting_provider: None,
        rename_provider: None,
        color_provider: None,
        folding_range_provider: None,
        execute_command_provider: None,
        workspace: None,
    }
}

// Requests

fn handle_request(
    writer: &mut impl Write,
    ctx: &mut ServerContext,
    msg: RequestMessage,
) -> Result<()> {
    let id = msg.id;
    let method = msg.method.as_str();

    let res = match method {
        "initialize" => initialize_request(),
        "shutdown" => shutdown_request(ctx),
        _ => unimplemented!(),
    };
    match res {
        Ok(res) => write_success_response(writer, id, res)?,
        Err(error) => write_error_response(writer, id, error)?,
    }
    Ok(())
}

type MessageResult<T> = std::result::Result<T, ResponseError>;

fn initialize_request() -> MessageResult<Value> {
    // The server has been initialized already.
    let error_message = "Unexpected initialize message".to_owned();
    Err(ResponseError::new(
        ErrorCodes::ServerNotInitialized,
        error_message,
    ))
}

fn shutdown_request(ctx: &mut ServerContext) -> MessageResult<Value> {
    ctx.state = State::ShuttingDown;
    Ok(Value::Null)
}

// Notifications

fn get_params<P: serde::de::DeserializeOwned>(params: Value) -> Result<P> {
    serde_json::from_value::<P>(params).map_err(|err| Error::ProtocolError(err.to_string()))
}

fn handle_notification(
    _write: &mut impl Write,
    ctx: &mut ServerContext,
    msg: NotificationMessage,
) -> Result<()> {
    let method = msg.method.as_str();
    eprintln!("Got notification: {}", method);

    use lsp_types::notification::*;
    match msg.method.as_str() {
        Exit::METHOD => exit_notification(ctx),
        DidOpenTextDocument::METHOD => {
            get_params(msg.params).and_then(|params| did_open_text_document(params))
        }
        DidChangeTextDocument::METHOD => {
            get_params(msg.params).and_then(|params| did_change_text_document(params))
        }
        _ => unimplemented!(),
    }
}

fn exit_notification(ctx: &mut ServerContext) -> Result<()> {
    // https://microsoft.github.io/language-server-protocol/specification#exit
    if ctx.state == State::ShuttingDown {
        ctx.exit_code = Some(0);
    } else {
        ctx.exit_code = Some(1);
    }
    Ok(())
}

fn did_open_text_document(_params: lsp_types::DidOpenTextDocumentParams) -> Result<()> {
    use lsp_types::notification::Notification;
    eprintln!(
        "Received {}: {:?}",
        lsp_types::notification::DidOpenTextDocument::METHOD,
        _params.text_document
    );
    Ok(())
}

fn did_change_text_document(_params: lsp_types::DidChangeTextDocumentParams) -> Result<()> {
    use lsp_types::notification::Notification;
    eprintln!(
        "Received {}: {:?}",
        lsp_types::notification::DidChangeTextDocument::METHOD,
        _params.text_document
    );
    Ok(())
}

// Initialization

fn initialize(
    reader: &mut impl BufRead,
    writer: &mut impl Write,
) -> Result<lsp_types::InitializeParams> {
    let message = read_message(reader)?;

    let (id, params) = match message {
        Message::Request(req) => req.cast::<lsp_types::request::Initialize>()?,
        _ => {
            // TODO: Gracefully handle `exit` and `shutdown` messages.
            let error_message = format!("Expected initialize message but got {:?}", message);
            return Err(Error::ProtocolError(error_message));
        }
    };

    let capabilities = create_server_capabilities();
    let res = lsp_types::InitializeResult {
        capabilities: capabilities,
    };
    write_success_result(writer, id, res)?;

    let message = read_message(reader)?;
    match message {
        Message::Notofication(notif) => notif.cast::<lsp_types::notification::Initialized>()?,
        _ => {
            let error_message = format!("Expected initialized message but got {:?}", message);
            return Err(Error::ProtocolError(error_message));
        }
    };

    Ok(params)
}

// Returns exit code.
pub fn start() -> Result<i32> {
    let mut reader = BufReader::new(io::stdin());
    let mut writer = BufWriter::new(io::stdout());

    let _params = initialize(&mut reader, &mut writer)?;

    let mut ctx = ServerContext::new();

    loop {
        eprintln!("Reading message...");
        let message = read_message(&mut reader)?;
        match message {
            Message::Request(request) => handle_request(&mut writer, &mut ctx, request)?,
            Message::Notofication(notification) => {
                handle_notification(&mut writer, &mut ctx, notification)?
            }
            _ => unimplemented!(),
        };

        if let Some(exit_code) = ctx.exit_code {
            return Ok(exit_code);
        }
    }
}

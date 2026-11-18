//! Private stdio bridge. The launcher supplies an immutable conversation scope;
//! models cannot select another desktop or change the scope in tool arguments.
use crate::store::Result;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
pub fn tools() -> Value {
    json!([
    {"name":"desktop_screenshot","description":"See the desktop attached to this conversation.","inputSchema":{"type":"object","properties":{},"additionalProperties":false}},
    {"name":"desktop_click","description":"Click the attached desktop. Human takeover pauses input.","inputSchema":{"type":"object","properties":{"x":{"type":"integer","minimum":0},"y":{"type":"integer","minimum":0},"button":{"type":"integer","enum":[1,2,3]}},"required":["x","y"],"additionalProperties":false}},
    {"name":"desktop_move","description":"Move the pointer on the attached desktop.","inputSchema":{"type":"object","properties":{"x":{"type":"integer","minimum":0},"y":{"type":"integer","minimum":0}},"required":["x","y"],"additionalProperties":false}},
    {"name":"desktop_scroll","description":"Scroll at a position on the attached desktop.","inputSchema":{"type":"object","properties":{"x":{"type":"integer","minimum":0},"y":{"type":"integer","minimum":0},"direction":{"type":"string","enum":["up","down"]},"steps":{"type":"integer","minimum":1,"maximum":12}},"required":["x","y","direction"],"additionalProperties":false}},
    {"name":"desktop_type","description":"Type Unicode text into the attached desktop.","inputSchema":{"type":"object","properties":{"text":{"type":"string","maxLength":16000}},"required":["text"],"additionalProperties":false}},
    {"name":"desktop_key","description":"Press a key or combination, such as ctrl+l or Return.","inputSchema":{"type":"object","properties":{"key":{"type":"string","maxLength":128}},"required":["key"],"additionalProperties":false}}
    ])
}
pub fn method(name: &str) -> Result<&'static str> {
    match name {
        "desktop_screenshot" => Ok("tool.screenshot"),
        "desktop_click" => Ok("tool.click"),
        "desktop_move" => Ok("tool.move"),
        "desktop_scroll" => Ok("tool.scroll"),
        "desktop_type" => Ok("tool.type"),
        "desktop_key" => Ok("tool.key"),
        _ => Err("Unknown desktop tool".into()),
    }
}
#[cfg(unix)]
pub fn invoke(socket: &str, session: &str, name: &str, args: &Value) -> Result<Value> {
    use std::os::unix::net::UnixStream;
    let mut stream = UnixStream::connect(socket).map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(15)))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(std::time::Duration::from_secs(5)))
        .map_err(|e| e.to_string())?;
    writeln!(
        stream,
        "{}",
        json!({"session":session,"method":method(name)?,"params":args})
    )
    .map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new(stream.take(32 * 1024 * 1024))
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    let value: Value = serde_json::from_str(&line).map_err(|e| e.to_string())?;
    if value["ok"] != true {
        return Err(value["error"]
            .as_str()
            .unwrap_or("Desktop operation failed")
            .into());
    }
    Ok(value["payload"].clone())
}
pub fn serve(socket: &str, session: &str) -> Result<()> {
    if session.is_empty() || session.len() > 512 {
        return Err("D4C_SESSION must identify the conversation".into());
    }
    let mut stdin = std::io::stdin().lock();
    let mut line = String::new();
    loop {
        line.clear();
        if (&mut stdin)
            .take(1024 * 1024 + 1)
            .read_line(&mut line)
            .map_err(|e| e.to_string())?
            == 0
        {
            return Ok(());
        }
        if line.len() > 1024 * 1024 {
            return Err("MCP request too large".into());
        }
        let request: Value = serde_json::from_str(&line).map_err(|e| e.to_string())?;
        let Some(id) = request.get("id") else {
            continue;
        };
        let result = match request["method"].as_str() {
            Some("initialize") => Ok(
                json!({"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"d4c-desktops","version":env!("CARGO_PKG_VERSION")}}),
            ),
            Some("ping") => Ok(json!({})),
            Some("tools/list") => Ok(json!({"tools":tools()})),
            Some("tools/call") => Ok(
                match invoke(
                    socket,
                    session,
                    request["params"]["name"].as_str().unwrap_or(""),
                    &request["params"]["arguments"],
                ) {
                    Ok(v) => v,
                    Err(e) => json!({"isError":true,"content":[{"type":"text","text":e}]}),
                },
            ),
            _ => Err("Unknown MCP method".to_string()),
        };
        let response = match result {
            Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
            Err(message) => {
                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":message}})
            }
        };
        println!("{response}");
        std::io::stdout().flush().map_err(|e| e.to_string())?;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn desktop_tools_cannot_choose_scope() {
        for t in tools().as_array().unwrap() {
            assert_eq!(t["inputSchema"]["additionalProperties"], false);
            assert!(t["inputSchema"]["properties"].get("session").is_none());
            assert!(t["inputSchema"]["properties"].get("desktopId").is_none());
            assert!(method(t["name"].as_str().unwrap()).is_ok());
        }
        assert!(method("desktop.start").is_err());
    }
}

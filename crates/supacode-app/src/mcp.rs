//! Serveur MCP local de SupaCode (transport HTTP « streamable », réponses JSON simples).
//! Les agents lancés par SupaCode s'y connectent pour piloter le navigateur intégré.
//!
//! Il n'écoute que sur 127.0.0.1, derrière un chemin secret tiré au hasard, et refuse
//! toute requête venant d'une page web (en-tête `Origin`).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use base64::Engine as _;
use serde_json::{Value, json};

/// Ce qu'un outil renvoie à l'agent.
pub enum Content {
    Text(String),
    Image(Vec<u8>),
}

/// Appel d'outil transmis à l'interface ; la réponse revient par `reply`.
pub struct Call {
    pub tool: String,
    pub args: Value,
    pub reply: async_channel::Sender<Result<Vec<Content>>>,
}

pub struct Server {
    pub url: String,
    pub calls: async_channel::Receiver<Call>,
}

pub fn start() -> Result<Server> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let token = uuid::Uuid::new_v4().simple().to_string();
    let path = format!("/mcp/{token}");
    let (sender, calls) = async_channel::unbounded();
    std::thread::Builder::new().name("supacode-mcp".into()).spawn(move || {
        for stream in listener.incoming().flatten() {
            let (sender, path) = (sender.clone(), path.clone());
            std::thread::spawn(move || {
                let _ = serve(stream, &path, &sender);
            });
        }
    })?;
    Ok(Server { url: format!("http://127.0.0.1:{port}/mcp/{token}"), calls })
}

/// Fichier `--mcp-config` donné à `claude`.
pub fn write_claude_config(url: &str) -> Option<PathBuf> {
    let file = dirs::config_dir()?.join("SupaCode").join("mcp.json");
    std::fs::create_dir_all(file.parent()?).ok()?;
    let config = json!({ "mcpServers": { "supacode": { "type": "http", "url": url } } });
    std::fs::write(&file, serde_json::to_string_pretty(&config).ok()?).ok()?;
    Some(file)
}

fn serve(stream: TcpStream, path: &str, calls: &async_channel::Sender<Call>) -> Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let mut length = 0;
    let mut origin = false;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header)? == 0 || header.trim().is_empty() {
            break;
        }
        let (name, value) = header.split_once(':').unwrap_or((&header, ""));
        match name.trim().to_ascii_lowercase().as_str() {
            "content-length" => length = value.trim().parse().unwrap_or(0),
            "origin" => origin = true,
            _ => {}
        }
    }
    let mut stream = stream;
    if target != path || origin {
        return respond(&mut stream, "403 Forbidden", None);
    }
    match method {
        "POST" => {}
        "DELETE" => return respond(&mut stream, "200 OK", None),
        _ => return respond(&mut stream, "405 Method Not Allowed", None),
    }
    let mut body = vec![0; length.min(16 << 20)];
    reader.read_exact(&mut body)?;
    let Ok(message) = serde_json::from_slice::<Value>(&body) else {
        return respond(&mut stream, "400 Bad Request", None);
    };
    match handle(&message, calls) {
        Some(answer) => respond(&mut stream, "200 OK", Some(&answer)),
        // Notification : rien à répondre.
        None => respond(&mut stream, "202 Accepted", None),
    }
}

fn respond(stream: &mut TcpStream, status: &str, body: Option<&Value>) -> Result<()> {
    let body = body.map(|b| b.to_string()).unwrap_or_default();
    let kind = if body.is_empty() { "" } else { "Content-Type: application/json; charset=utf-8\r\n" };
    write!(stream, "HTTP/1.1 {status}\r\n{kind}Content-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())?;
    stream.flush()?;
    Ok(())
}

fn handle(message: &Value, calls: &async_channel::Sender<Call>) -> Option<Value> {
    let id = message.get("id")?.clone();
    let method = message.get("method").and_then(Value::as_str).unwrap_or("");
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": params.get("protocolVersion").and_then(Value::as_str).unwrap_or("2025-06-18"),
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "supacode", "version": env!("CARGO_PKG_VERSION") },
            "instructions": "Navigateur intégré à SupaCode, affiché à l'utilisateur dans le panneau de droite. \
                Sers-t'en pour vérifier une fonctionnalité, reproduire un bug ou montrer un résultat. \
                Commence par browser_open, puis browser_snapshot pour voir les éléments (numérotés par « ref »).",
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools() })),
        "tools/call" => {
            let tool = params.get("name").and_then(Value::as_str).unwrap_or("").to_owned();
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            Ok(call(calls, tool, args))
        }
        _ => Err(json!({ "code": -32601, "message": format!("méthode inconnue : {method}") })),
    };
    Some(match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err(error) => json!({ "jsonrpc": "2.0", "id": id, "error": error }),
    })
}

fn call(calls: &async_channel::Sender<Call>, tool: String, args: Value) -> Value {
    let (reply, answer) = async_channel::bounded(1);
    let outcome = calls
        .send_blocking(Call { tool, args, reply })
        .map_err(|_| anyhow::anyhow!("SupaCode est en train de se fermer"))
        .and_then(|_| answer.recv_blocking().map_err(|_| anyhow::anyhow!("l'appel a été abandonné")))
        .and_then(|r| r);
    match outcome {
        Ok(contents) => json!({
            "content": contents.into_iter().map(|c| match c {
                Content::Text(text) => json!({ "type": "text", "text": text }),
                Content::Image(png) => json!({ "type": "image", "mimeType": "image/png", "data": base64::engine::general_purpose::STANDARD.encode(png) }),
            }).collect::<Vec<_>>(),
        }),
        Err(error) => json!({ "content": [{ "type": "text", "text": format!("Erreur : {error:#}") }], "isError": true }),
    }
}

fn tools() -> Value {
    let target = json!({
        "ref": { "type": "integer", "description": "Numéro « ref » donné par browser_snapshot." },
        "selector": { "type": "string", "description": "Sélecteur CSS, à défaut de ref." },
    });
    let with = |extra: Value| {
        let mut properties = target.clone();
        properties.as_object_mut().unwrap().extend(extra.as_object().cloned().unwrap_or_default());
        properties
    };
    json!([
        {
            "name": "browser_open",
            "description": "Ouvre une adresse dans le navigateur de SupaCode (visible par l'utilisateur) et attend la fin du chargement.",
            "inputSchema": { "type": "object", "properties": { "url": { "type": "string", "description": "Adresse, par exemple http://localhost:3000" } }, "required": ["url"] },
        },
        {
            "name": "browser_snapshot",
            "description": "Décrit la page affichée : adresse, titre, texte visible et éléments interactifs numérotés (ref) à utiliser avec browser_click et browser_type.",
            "inputSchema": { "type": "object", "properties": {} },
        },
        {
            "name": "browser_screenshot",
            "description": "Capture d'écran de la partie visible de la page.",
            "inputSchema": { "type": "object", "properties": {} },
        },
        {
            "name": "browser_click",
            "description": "Clique sur un élément (vrai clic de souris au centre de l'élément).",
            "inputSchema": { "type": "object", "properties": target },
        },
        {
            "name": "browser_type",
            "description": "Saisit du texte dans un champ (remplace son contenu). submit=true appuie ensuite sur Entrée.",
            "inputSchema": { "type": "object", "properties": with(json!({ "text": { "type": "string" }, "submit": { "type": "boolean" } })), "required": ["text"] },
        },
        {
            "name": "browser_select",
            "description": "Choisit une option d'une liste déroulante <select> (par valeur ou texte affiché).",
            "inputSchema": { "type": "object", "properties": with(json!({ "value": { "type": "string" } })), "required": ["value"] },
        },
        {
            "name": "browser_press",
            "description": "Appuie sur une touche dans la page : Enter, Escape, Tab, ArrowDown, Backspace…",
            "inputSchema": { "type": "object", "properties": { "key": { "type": "string" } }, "required": ["key"] },
        },
        {
            "name": "browser_wait",
            "description": "Attend qu'un texte ou un sélecteur apparaisse dans la page (10 s par défaut).",
            "inputSchema": { "type": "object", "properties": { "text": { "type": "string" }, "selector": { "type": "string" }, "timeout_ms": { "type": "integer" } } },
        },
        {
            "name": "browser_eval",
            "description": "Exécute du JavaScript dans la page et renvoie le résultat en JSON. Une expression, ou un corps de fonction avec return. Pas d'await.",
            "inputSchema": { "type": "object", "properties": { "script": { "type": "string" } }, "required": ["script"] },
        },
        {
            "name": "browser_console",
            "description": "Messages de la console de la page (erreurs comprises), du plus ancien au plus récent.",
            "inputSchema": { "type": "object", "properties": { "errors_only": { "type": "boolean" } } },
        },
        {
            "name": "browser_navigate",
            "description": "Page précédente, suivante, ou rechargement.",
            "inputSchema": { "type": "object", "properties": { "action": { "type": "string", "enum": ["back", "forward", "reload"] } }, "required": ["action"] },
        },
    ])
}

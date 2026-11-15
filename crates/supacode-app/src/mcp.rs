//! Serveur MCP local de SupaCode (transport HTTP « streamable », réponses JSON simples).
//! Les agents lancés par SupaCode s'y connectent pour piloter le navigateur intégré et pour
//! se coordonner entre fenêtres d'agents.
//!
//! L'adresse peut porter `?agent=N` (fenêtre d'agent qui appelle) et `browser=0` (navigateur
//! interdit par les règles d'équipe ou désactivé).
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
    /// Fenêtre d'agent à l'origine de l'appel, si l'adresse l'indique.
    pub agent: Option<usize>,
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
    // Lecture et écriture sur la même connexion, sans la dupliquer : une copie (try_clone) pouvait
    // être héritée par un programme lancé au même moment (git, claude) et garder la connexion ouverte.
    let mut reader = BufReader::new(&stream);
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
    // Le corps est lu avant toute réponse, même un refus : fermer la connexion avec des données
    // non lues fait envoyer un « reset » par Windows, et le client perd la réponse.
    let mut body = vec![0; length.min(16 << 20)];
    reader.read_exact(&mut body)?;
    let mut stream = &stream;
    let (target, query) = target.split_once('?').unwrap_or((target, ""));
    // Hook de Claude Code : `<chemin>/hook/<événement>/<agent>` (voir `hook_command`).
    let hook = target.strip_prefix(path).and_then(|rest| rest.strip_prefix("/hook/")).and_then(|rest| rest.split_once('/')).and_then(|(event, agent)| Some((event.to_owned(), agent.parse::<usize>().ok()?)));
    if (target != path && hook.is_none()) || origin {
        return respond(&mut stream, "403 Forbidden", None);
    }
    // Séparateur `,` accepté : `&` serait interprété par `cmd.exe` quand `claude` est un script.
    let param = |key: &str| query.split(['&', ',']).find_map(|pair| pair.strip_prefix(key).and_then(|v| v.strip_prefix('=')));
    let access = Access { agent: param("agent").and_then(|v| v.parse().ok()), browser: param("browser") != Some("0") };
    match method {
        "POST" => {}
        "DELETE" => return respond(&mut stream, "200 OK", None),
        _ => return respond(&mut stream, "405 Method Not Allowed", None),
    }
    // Hook de Claude Code (voir `hook_command`) : messages des autres agents en attente.
    if let Some((event, agent)) = hook {
        let text = match call(calls, Some(agent), INBOX_TOOL.into(), json!({ "event": event })) {
            value if value.get("isError").is_some() => String::new(),
            value => value["content"].as_array().into_iter().flatten().filter_map(|c| c["text"].as_str()).collect::<Vec<_>>().join("\n"),
        };
        return respond_text(&mut stream, &hook_output(&event, &text));
    }
    let Ok(message) = serde_json::from_slice::<Value>(&body) else {
        return respond(&mut stream, "400 Bad Request", None);
    };
    match handle(&message, calls, &access) {
        Some(answer) => respond(&mut stream, "200 OK", Some(&answer)),
        // Notification : rien à répondre.
        None => respond(&mut stream, "202 Accepted", None),
    }
}

/// Longueur maximale d'un message entre agents : ils échangent des faits, pas des discussions.
pub const MAX_MESSAGE: usize = 300;

/// Outil interne (pas proposé aux agents) qui vide la boîte de réception d'un agent.
pub const INBOX_TOOL: &str = "agent_inbox";
/// Outil qui crée des sous-agents dans la grille (exécuté avec la fenêtre de l'application).
pub const SPAWN_TOOL: &str = "agents_spawn";
/// Sous-agents créés en un appel, au plus.
pub const MAX_SPAWN: usize = 6;

/// Hooks `PostToolUse` et `Stop` (type `http`) donnés à chaque agent : après chaque action,
/// et avant de s'arrêter, Claude Code demande à SupaCode les messages en attente ; Claude les
/// lit tout de suite, en pleine tâche, sans que rien soit tapé dans son terminal.
pub fn agent_hooks(url: &str, agent: usize) -> Value {
    let hook = |event: &str| json!([{ "hooks": [{ "type": "http", "url": format!("{url}/hook/{event}/{agent}"), "timeout": 10 }] }]);
    json!({ "hooks": { "PostToolUse": hook("post"), "Stop": hook("stop") } })
}

/// Réponse attendue par Claude Code pour un hook ; vide s'il n'y a rien à transmettre.
fn hook_output(event: &str, text: &str) -> String {
    if text.trim().is_empty() {
        return String::new();
    }
    let text = format!("Message d'un autre agent SupaCode, à prendre en compte maintenant :\n{text}");
    let value = if event == "stop" {
        // Empêche l'agent de s'arrêter : il traite d'abord le message.
        json!({ "decision": "block", "reason": text })
    } else {
        json!({ "hookSpecificOutput": { "hookEventName": "PostToolUse", "additionalContext": text } })
    };
    value.to_string()
}

fn respond_text(stream: &mut impl Write, body: &str) -> Result<()> {
    write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())?;
    stream.flush()?;
    Ok(())
}

fn respond(stream: &mut impl Write, status: &str, body: Option<&Value>) -> Result<()> {
    let body = body.map(|b| b.to_string()).unwrap_or_default();
    let kind = if body.is_empty() { "" } else { "Content-Type: application/json; charset=utf-8\r\n" };
    write!(stream, "HTTP/1.1 {status}\r\n{kind}Content-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())?;
    stream.flush()?;
    Ok(())
}

/// Ce que l'adresse appelée autorise.
struct Access {
    agent: Option<usize>,
    browser: bool,
}

fn handle(message: &Value, calls: &async_channel::Sender<Call>, access: &Access) -> Option<Value> {
    let id = message.get("id")?.clone();
    let method = message.get("method").and_then(Value::as_str).unwrap_or("");
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": params.get("protocolVersion").and_then(Value::as_str).unwrap_or("2025-06-18"),
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "supacode", "version": env!("CARGO_PKG_VERSION") },
            "instructions": instructions(access),
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools(access) })),
        "tools/call" => {
            let tool = params.get("name").and_then(Value::as_str).unwrap_or("").to_owned();
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            if tool.starts_with("browser_") && !access.browser {
                Ok(json!({ "content": [{ "type": "text", "text": "Erreur : le navigateur n'est pas autorisé pour cette session." }], "isError": true }))
            } else {
                Ok(call(calls, access.agent, tool, args))
            }
        }
        _ => Err(json!({ "code": -32601, "message": format!("méthode inconnue : {method}") })),
    };
    Some(match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err(error) => json!({ "jsonrpc": "2.0", "id": id, "error": error }),
    })
}

fn call(calls: &async_channel::Sender<Call>, agent: Option<usize>, tool: String, args: Value) -> Value {
    let (reply, answer) = async_channel::bounded(1);
    let outcome = calls
        .send_blocking(Call { agent, tool, args, reply })
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

fn instructions(access: &Access) -> String {
    let mut text = String::new();
    if access.browser {
        text.push_str(
            "Navigateur intégré à SupaCode, affiché à l'utilisateur dans le panneau de droite. \
             Sers-t'en pour vérifier une fonctionnalité, reproduire un bug ou montrer un résultat. \
             Commence par browser_open, puis browser_snapshot pour voir les éléments (numérotés par « ref »). ",
        );
    }
    if access.agent.is_some() {
        text.push_str(
            "Plusieurs agents peuvent tourner dans SupaCode, chacun dans sa fenêtre. agents_list les montre \
             avec leur état, agent_read affiche l'écran d'un agent et agent_send lui écrit. agents_spawn crée \
             des sous-agents dans la grille : de vrais terminaux que l'utilisateur voit et pilote, chacun avec le \
             modèle adapté à sa tâche.",
        );
    }
    text
}

/// Noms des outils que le serveur donne à une fenêtre d'agent (affichés dans l'onglet Extensions).
pub fn agent_tool_names(browser: bool) -> Vec<String> {
    let access = Access { agent: Some(0), browser };
    tools(&access).as_array().into_iter().flatten().filter_map(|t| t.get("name").and_then(Value::as_str).map(str::to_owned)).collect()
}

fn tools(access: &Access) -> Value {
    let mut all = Vec::new();
    if access.browser {
        all.extend(browser_tools().as_array().cloned().unwrap_or_default());
    }
    if access.agent.is_some() {
        all.extend(agent_tools().as_array().cloned().unwrap_or_default());
    }
    Value::Array(all)
}

fn agent_tools() -> Value {
    json!([
        {
            "name": "agents_list",
            "description": "Liste les fenêtres d'agents ouvertes dans SupaCode : numéro, sujet en cours, projet et état (au travail, attend une réponse, au repos, terminée). Ta propre fenêtre est marquée « toi ».",
            "inputSchema": { "type": "object", "properties": {} },
        },
        {
            "name": "agent_read",
            "description": "Texte affiché en ce moment dans la fenêtre d'un autre agent, pour voir où il en est.",
            "inputSchema": { "type": "object", "properties": { "id": { "type": "integer", "description": "Numéro de la fenêtre (agents_list)." } }, "required": ["id"] },
        },
        {
            "name": "agent_send",
            "description": "Envoie un message court à un autre agent (300 caractères au plus), seulement s'il change son travail : fichier ou fonction que tu modifies, conflit, résultat dont il a besoin. Pas de politesse, de remerciement ni d'accusé de réception. Il le lit en direct dès sa prochaine action.",
            "inputSchema": { "type": "object", "properties": { "to": { "type": "integer", "description": "Numéro de la fenêtre destinataire (agents_list)." }, "message": { "type": "string", "maxLength": 300, "description": "Une ou deux phrases factuelles." } }, "required": ["to", "message"] },
        },
        {
            "name": SPAWN_TOOL,
            "description": "Crée des sous-agents dans la grille de SupaCode, pour mener en parallèle les parties indépendantes d'une grosse tâche. Chacun est un vrai terminal (Claude Code ou Codex) que l'utilisateur voit travailler et peut piloter : il peut lui écrire, le corriger ou l'arrêter. SupaCode choisit l'agent et le modèle de chacun selon sa tâche (orchestrateur du projet et règles d'équipe), sauf si tu les imposes. Chaque sous-agent t'envoie son résultat avec agent_send ; suis-les avec agents_list et agent_read. Ne découpe pas une petite tâche.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agents": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": MAX_SPAWN,
                        "items": {
                            "type": "object",
                            "properties": {
                                "title": { "type": "string", "description": "Titre court affiché sur la tuile (quelques mots)." },
                                "task": { "type": "string", "description": "Consigne complète et autonome : le sous-agent ne voit pas ta conversation (fichiers, contraintes, ce qu'il doit te renvoyer)." },
                                "agent": { "type": "string", "enum": ["auto", "claude", "codex"], "description": "auto (défaut) : choisi par l'orchestrateur." },
                                "model": { "type": "string", "description": "haiku, sonnet, opus… ou auto (défaut) : choisi selon la tâche." }
                            },
                            "required": ["task"]
                        }
                    }
                },
                "required": ["agents"]
            },
        },
    ])
}

fn browser_tools() -> Value {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// POST brut, renvoie le corps de la réponse.
    fn post(url: &str, body: &str) -> String {
        let rest = url.strip_prefix("http://").unwrap();
        let (host, path) = rest.split_once('/').unwrap();
        let mut stream = TcpStream::connect(host).unwrap();
        write!(stream, "POST /{path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).unwrap();
        answer.split_once("\r\n\r\n").unwrap().1.to_owned()
    }

    #[test]
    fn les_agents_peuvent_creer_des_sous_agents() {
        let names = agent_tool_names(false);
        assert!(names.iter().any(|n| n == SPAWN_TOOL));
        assert!(names.iter().any(|n| n == "agent_send"));
        assert!(!names.iter().any(|n| n.starts_with("browser_")));
        assert!(agent_tool_names(true).iter().any(|n| n == "browser_open"));
    }

    #[test]
    fn le_hook_transmet_les_messages_en_attente() {
        let server = start().unwrap();
        let calls = server.calls.clone();
        std::thread::spawn(move || {
            while let Ok(call) = calls.recv_blocking() {
                assert_eq!(call.tool, INBOX_TOOL);
                let text = if call.agent == Some(3) { "[Message de l'agent #1] j'ai déjà créé utils.rs" } else { "" };
                let _ = call.reply.send_blocking(Ok(vec![Content::Text(text.into())]));
            }
        });
        let hooks = agent_hooks(&server.url, 3);
        let post_url = hooks["hooks"]["PostToolUse"][0]["hooks"][0]["url"].as_str().unwrap().to_owned();
        let answer: Value = serde_json::from_str(&post(&post_url, r#"{"hook_event_name":"PostToolUse"}"#)).unwrap();
        assert_eq!(answer["hookSpecificOutput"]["hookEventName"], "PostToolUse");
        assert!(answer["hookSpecificOutput"]["additionalContext"].as_str().unwrap().contains("utils.rs"));

        let stop_url = hooks["hooks"]["Stop"][0]["hooks"][0]["url"].as_str().unwrap().to_owned();
        let answer: Value = serde_json::from_str(&post(&stop_url, "{}")).unwrap();
        assert_eq!(answer["decision"], "block");

        // Rien en attente : réponse vide (Claude Code continue normalement).
        assert_eq!(post(&post_url.replace("/post/3", "/post/4"), "{}"), "");
        // Chemin secret faux : refusé.
        assert_eq!(post(&post_url.replace("/hook/", "/x/hook/"), "{}"), "");
    }
}

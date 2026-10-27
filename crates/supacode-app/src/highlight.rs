//! Coloration syntaxique de l'aperçu du code (syntect + grammaires étendues de two-face),
//! avec les couleurs de code de la maquette.

use std::ops::Range;
use std::path::Path;
use std::sync::OnceLock;

use gpui_kit::{HighlightStyle, Rgba, StyledText, rgb};
use syntect::parsing::{ParseState, ScopeStack, ScopeStackOp, SyntaxReference, SyntaxSet};

/// Couleurs par portion de ligne, en octets.
pub type Spans = Vec<(Range<usize>, Rgba)>;

/// Au-delà, le fichier s'affiche sans couleurs (l'analyse resterait rapide, mais inutile pour un aperçu).
const MAX_LINES: usize = 10_000;

struct Palette {
    keyword: Rgba,
    function: Rgba,
    string: Rgba,
    number: Rgba,
    comment: Rgba,
    kind: Rgba,
}

fn colors() -> Palette {
    Palette {
        keyword: rgb(0xB9A2FF),
        function: rgb(0x7DB8FF),
        string: rgb(0xA5D387),
        number: rgb(0xF2A66E),
        comment: rgb(0x868B96),
        kind: rgb(0xE7C77D),
    }
}

fn syntaxes() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(two_face::syntax::extra_newlines)
}

fn syntax_for(path: &str) -> Option<&'static SyntaxReference> {
    let set = syntaxes();
    let path = Path::new(path);
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_default();
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    set.find_syntax_by_extension(ext).or_else(|| set.find_syntax_by_extension(name)).filter(|s| s.name != "Plain Text")
}

/// Couleur d'une pile de portées : la portée la plus précise qui a une couleur l'emporte.
fn color_for(stack: &ScopeStack, colors: &Palette) -> Option<Rgba> {
    stack.as_slice().iter().rev().find_map(|scope| {
        let name = scope.build_string();
        let starts = |prefix: &str| name.starts_with(prefix);
        if starts("comment") {
            Some(colors.comment)
        } else if starts("string") || starts("constant.character") {
            Some(colors.string)
        } else if starts("constant.numeric") || starts("constant.language") {
            Some(colors.number)
        } else if starts("entity.name.function") || starts("support.function") || starts("meta.function-call.identifier") || starts("variable.function") {
            Some(colors.function)
        } else if starts("entity.name.type") || starts("entity.name.class") || starts("entity.name.struct") || starts("entity.name.enum") || starts("support.type") || starts("support.class") || starts("entity.other.inherited-class") {
            Some(colors.kind)
        } else if starts("keyword") && !starts("keyword.operator") || starts("storage") || starts("variable.language") {
            Some(colors.keyword)
        } else {
            None
        }
    })
}

/// Couleurs de chaque ligne ; vide si le langage est inconnu.
pub fn highlight<S: AsRef<str>>(path: &str, lines: &[S]) -> Vec<Spans> {
    let Some(syntax) = syntax_for(path).filter(|_| lines.len() <= MAX_LINES) else {
        return Vec::new();
    };
    let set = syntaxes();
    let colors = colors();
    let mut state = ParseState::new(syntax);
    let mut stack = ScopeStack::new();
    let mut out = Vec::with_capacity(lines.len());
    for line in lines {
        let line = line.as_ref();
        // Les grammaires « newlines » attendent la fin de ligne.
        let text = format!("{line}\n");
        let ops = state.parse_line(&text, set).unwrap_or_default();
        let mut spans: Spans = Vec::new();
        let mut last = 0;
        let push = |range: Range<usize>, stack: &ScopeStack, spans: &mut Spans| {
            let range = range.start.min(line.len())..range.end.min(line.len());
            if range.is_empty() {
                return;
            }
            if let Some(color) = color_for(stack, &colors) {
                match spans.last_mut() {
                    Some((prev, c)) if prev.end == range.start && *c == color => prev.end = range.end,
                    _ => spans.push((range, color)),
                }
            }
        };
        for (offset, op) in ops {
            push(last..offset, &stack, &mut spans);
            last = offset;
            apply(&mut stack, &op);
        }
        push(last..text.len(), &stack, &mut spans);
        out.push(spans);
    }
    out
}

/// Ligne de code colorée (une ligne vide garde sa hauteur).
pub fn code_text(text: &str, spans: &Spans) -> StyledText {
    if text.is_empty() {
        return StyledText::new(" ");
    }
    let highlights = spans.iter().map(|(range, color)| (range.clone(), HighlightStyle { color: Some((*color).into()), ..Default::default() }));
    StyledText::new(text.to_owned()).with_highlights(highlights.collect::<Vec<_>>())
}

/// Tabulations remplacées par 4 espaces, comme à l'affichage.
pub fn expand_tabs(text: &str) -> String {
    text.replace('\t', "    ")
}

fn apply(stack: &mut ScopeStack, op: &ScopeStackOp) {
    // Une erreur de pile (grammaire incohérente) ne doit pas casser l'aperçu.
    let _ = stack.apply(op);
}

#[cfg(test)]
mod tests {
    use super::highlight;

    #[test]
    fn colore_rust_et_typescript() {
        let rust = highlight("main.rs", &["fn main() {", "    // bonjour", "    let x = \"a\";", "}"]);
        assert_eq!(rust.len(), 4);
        assert!(!rust[0].is_empty(), "mot-clé fn");
        assert!(!rust[1].is_empty(), "commentaire");
        let ts = highlight("app.ts", &["const x: number = 42;"]);
        assert!(!ts[0].is_empty());
        assert!(highlight("notes.inconnu", &["texte"]).is_empty());
    }
}

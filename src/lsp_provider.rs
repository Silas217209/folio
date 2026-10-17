use std::{ops::Range, rc::Rc, time::Duration};

use anyhow::Result;
use gpui_kit::{
    App, Entity, SharedString, Task, Window,
    component::input::{
        CodeActionProvider, CompletionProvider, DefinitionProvider, DocumentColorProvider,
        EditorState, HoverProvider, Rope,
    },
};
use lsp_types::{
    CodeAction, CompletionContext, CompletionResponse, CompletionTextEdit, Hover, LocationLink,
};
use serde_json::json;

use crate::lsp::{self, Client};

pub struct Provider {
    pub client: Client,
    pub uri: String,
}

impl Provider {
    pub fn new(client: Client, uri: String) -> Rc<Self> {
        Rc::new(Self { client, uri })
    }
    fn sync(&self, text: &Rope) -> String {
        let text = text.to_string();
        self.client.sync(&self.uri, &text);
        text
    }

    fn current(&self, text: &Rope) -> Result<String> {
        let text = text.to_string();
        // Delayed hover/color requests carry a captured rope. They must never
        // overwrite newer didChange text while a user is typing.
        if self
            .client
            .snapshot(&self.uri)
            .is_none_or(|snapshot| snapshot.text != text)
        {
            anyhow::bail!("The document changed before this request was sent");
        }
        Ok(text)
    }

    pub fn hover_now(
        &self,
        text: &Rope,
        offset: usize,
        cx: &mut App,
    ) -> Task<Result<Option<Hover>>> {
        self.request_hover(text, offset, Duration::ZERO, cx)
    }

    fn request_hover(
        &self,
        text: &Rope,
        offset: usize,
        delay: Duration,
        cx: &mut App,
    ) -> Task<Result<Option<Hover>>> {
        let text = match self.current(text) {
            Ok(text) => text,
            Err(error) => return Task::ready(Err(error)),
        };
        let client = self.client.clone();
        let uri = self.uri.clone();
        let timer = (!delay.is_zero()).then(|| cx.background_executor().timer(delay));
        cx.background_executor().spawn(async move {
            if let Some(timer) = timer { timer.await; }
            // A pause may outlive a source edit. Do not request or display
            // documentation for the captured version after it changes.
            let current = || client.snapshot(&uri).is_some_and(|snapshot| snapshot.text == text);
            if !current() { return Ok(None); }
            let value = client.request("textDocument/hover", json!({ "textDocument": { "uri": uri }, "position": lsp::byte_position(&text, offset) })).await?;
            if !current() { return Ok(None); }
            let mut hover: Option<Hover> = serde_json::from_value(value)?;
            if let Some(hover) = &mut hover { hover.range = hover.range.map(|range| lsp::editor_range(&text, range)); }
            Ok(hover)
        })
    }
}

impl CompletionProvider for Provider {
    fn completions(
        &self,
        text: &Rope,
        offset: usize,
        mut trigger: CompletionContext,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<Result<CompletionResponse>> {
        let text = self.sync(text);
        let client = self.client.clone();
        let uri = self.uri.clone();
        let capabilities = client.capabilities();
        let query = text[..offset]
            .rsplit(|ch: char| !ch.is_alphanumeric() && ch != '_' && ch != '-')
            .next()
            .unwrap_or("")
            .to_lowercase();
        let character = trigger
            .trigger_character
            .as_ref()
            .and_then(|query| query.chars().last())
            .map(|ch| ch.to_string());
        if character.as_ref().is_some_and(|ch| {
            capabilities["completionProvider"]["triggerCharacters"]
                .as_array()
                .is_some_and(|triggers| triggers.iter().any(|trigger| trigger == ch))
        }) {
            trigger.trigger_character = character;
        } else {
            trigger.trigger_character = None;
            trigger.trigger_kind = lsp_types::CompletionTriggerKind::INVOKED;
        }
        cx.background_executor().spawn(async move {
            let value = client.request("textDocument/completion", json!({ "textDocument": { "uri": uri }, "position": lsp::byte_position(&text, offset), "context": trigger })).await?;
            let mut response: CompletionResponse = if value.is_null() { CompletionResponse::Array(vec![]) } else { serde_json::from_value(value)? };
            let items = match &mut response { CompletionResponse::Array(items) => items, CompletionResponse::List(list) => &mut list.items };
            for item in items {
                if let Some(edit) = &mut item.text_edit {
                    match edit {
                        CompletionTextEdit::Edit(edit) => edit.range = lsp::editor_range(&text, edit.range),
                        CompletionTextEdit::InsertAndReplace(edit) => {
                            edit.insert = lsp::editor_range(&text, edit.insert);
                            edit.replace = lsp::editor_range(&text, edit.replace);
                        }
                    }
                }
                // Tinymist 0.15.10 emits snippets even when snippetSupport is
                // false. Kit inserts literal text, so expand placeholder defaults
                // rather than putting ${1:...} markers in the user's source.
                if item.insert_text_format == Some(lsp_types::InsertTextFormat::SNIPPET) {
                    if let Some(text) = &mut item.insert_text { *text = plain_snippet(text); }
                    if let Some(edit) = &mut item.text_edit {
                        let text = match edit { CompletionTextEdit::Edit(edit) => &mut edit.new_text, CompletionTextEdit::InsertAndReplace(edit) => &mut edit.new_text };
                        *text = plain_snippet(text);
                    }
                    item.insert_text_format = Some(lsp_types::InsertTextFormat::PLAIN_TEXT);
                }
            }
            let items = match &mut response { CompletionResponse::Array(items) => items, CompletionResponse::List(list) => &mut list.items };
            items.sort_by_key(|item| (!item.label.to_lowercase().starts_with(&query), item.sort_text.clone().unwrap_or_else(|| item.label.clone())));
            Ok(response)
        })
    }

    fn is_completion_trigger(&self, _: usize, new_text: &str, _: &mut App) -> bool {
        new_text
            .chars()
            .last()
            .is_some_and(|ch| ch.is_alphanumeric() || "_#.@:/\"(-,".contains(ch))
    }
}

fn plain_snippet(snippet: &str) -> String {
    fn expand(chars: &mut std::iter::Peekable<std::str::Chars<'_>>, nested: bool) -> String {
        let mut result = String::new();
        while let Some(ch) = chars.next() {
            match ch {
                '}' if nested => break,
                '\\' => {
                    if let Some(ch) = chars.next() {
                        result.push(ch);
                    }
                }
                '$' => {
                    if chars.peek() == Some(&'{') {
                        chars.next();
                        while chars
                            .peek()
                            .is_some_and(|ch| ch.is_alphanumeric() || *ch == '_')
                        {
                            chars.next();
                        }
                        match chars.next() {
                            Some(':') => result.push_str(&expand(chars, true)),
                            Some('|') => {
                                let mut first = true;
                                while let Some(ch) = chars.next() {
                                    match ch {
                                        '\\' => {
                                            if let Some(ch) = chars.next()
                                                && first
                                            {
                                                result.push(ch);
                                            }
                                        }
                                        ',' => first = false,
                                        '|' if chars.peek() == Some(&'}') => {
                                            chars.next();
                                            break;
                                        }
                                        ch if first => result.push(ch),
                                        _ => {}
                                    }
                                }
                            }
                            Some('}') | None => {}
                            _ => while chars.next().is_some_and(|ch| ch != '}') {},
                        }
                    } else if chars.peek().is_some_and(|ch| ch.is_ascii_digit()) {
                        while chars.peek().is_some_and(|ch| ch.is_ascii_digit()) {
                            chars.next();
                        }
                    } else {
                        result.push('$');
                    }
                }
                ch => result.push(ch),
            }
        }
        result
    }
    expand(&mut snippet.chars().peekable(), false)
}

impl HoverProvider for Provider {
    fn hover(
        &self,
        text: &Rope,
        offset: usize,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Option<Hover>>> {
        // Kit debounces the mouse for 150 ms before calling this provider.
        // Keep explicit keyboard requests immediate while mouse hovers wait
        // about 650 ms in total. Dropping the task cancels the pending request.
        self.request_hover(text, offset, Duration::from_millis(500), cx)
    }
}

impl DefinitionProvider for Provider {
    fn definitions(
        &self,
        text: &Rope,
        offset: usize,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Vec<LocationLink>>> {
        let text = self.sync(text);
        let client = self.client.clone();
        let uri = self.uri.clone();
        cx.background_executor().spawn(async move {
            let value = client.request("textDocument/definition", json!({ "textDocument": { "uri": uri }, "position": lsp::byte_position(&text, offset) })).await?;
            let response: Option<lsp_types::GotoDefinitionResponse> = serde_json::from_value(value)?;
            let mut links = match response {
                Some(lsp_types::GotoDefinitionResponse::Link(links)) => links,
                Some(lsp_types::GotoDefinitionResponse::Scalar(location)) => vec![location_link(location)],
                Some(lsp_types::GotoDefinitionResponse::Array(locations)) => locations.into_iter().map(location_link).collect(),
                None => vec![],
            };
            for link in &mut links { link.origin_selection_range = link.origin_selection_range.map(|range| lsp::editor_range(&text, range)); }
            // The host handles target positions in their original UTF-16 form,
            // including other files. Only origin ranges belong to this editor.
            Ok(links)
        })
    }
}

fn location_link(location: lsp_types::Location) -> LocationLink {
    LocationLink {
        origin_selection_range: None,
        target_uri: location.uri,
        target_range: location.range,
        target_selection_range: location.range,
    }
}

fn usable_code_action(action: &CodeAction, uri: &str, text: &str) -> bool {
    if action.title != "Create missing variable" {
        return true;
    }
    let Some(changes) = action.edit.as_ref().and_then(|edit| edit.changes.as_ref()) else {
        return true;
    };
    let Some((_, edits)) = changes.iter().find(|(target, _)| target.as_str() == uri) else {
        return true;
    };
    fn bindings(node: &typst_syntax::SyntaxNode) -> usize {
        usize::from(node.kind() == typst_syntax::SyntaxKind::LetBinding)
            + node.children().map(bindings).sum::<usize>()
    }
    // This Tinymist release sometimes emits bare `let` text into markup.
    // Offer its variable-creation fix only when it actually adds a declaration.
    lsp::edited_text(text, edits).is_ok_and(|updated| {
        bindings(&typst_syntax::parse(&updated)) > bindings(&typst_syntax::parse(text))
    })
}

impl DocumentColorProvider for Provider {
    fn document_colors(
        &self,
        text: &Rope,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Vec<lsp_types::ColorInformation>>> {
        let text = match self.current(text) {
            Ok(text) => text,
            Err(error) => return Task::ready(Err(error)),
        };
        let client = self.client.clone();
        let uri = self.uri.clone();
        cx.background_executor().spawn(async move {
            let value = client
                .request(
                    "textDocument/documentColor",
                    json!({ "textDocument": { "uri": uri } }),
                )
                .await?;
            let mut colors: Vec<lsp_types::ColorInformation> = if value.is_null() {
                vec![]
            } else {
                serde_json::from_value(value)?
            };
            for color in &mut colors {
                color.range = lsp::editor_range(&text, color.range);
            }
            Ok(colors)
        })
    }
}

impl CodeActionProvider for Provider {
    fn id(&self) -> SharedString {
        "tinymist".into()
    }
    fn code_actions(
        &self,
        state: Entity<EditorState>,
        range: Range<usize>,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Vec<CodeAction>>> {
        let text = self.sync(state.read(cx).text());
        let range = lsp_types::Range::new(
            lsp::byte_position(&text, range.start),
            lsp::byte_position(&text, range.end),
        );
        let client = self.client.clone();
        let uri = self.uri.clone();
        cx.background_executor().spawn(async move {
            let value = client.request("textDocument/codeAction", json!({ "textDocument": { "uri": uri }, "range": range, "context": { "diagnostics": client.diagnostics(&uri) } })).await?;
            let actions: Vec<lsp_types::CodeActionOrCommand> = if value.is_null() { vec![] } else { serde_json::from_value(value)? };
            Ok(actions.into_iter().map(|action| match action {
                lsp_types::CodeActionOrCommand::CodeAction(action) => action,
                lsp_types::CodeActionOrCommand::Command(command) => CodeAction { title: command.title.clone(), command: Some(command), ..Default::default() },
            }).filter(|action| action.disabled.is_none() && usable_code_action(action, &uri, &text)).collect())
        })
    }

    fn perform_code_action(
        &self,
        _: Entity<EditorState>,
        action: CodeAction,
        _: bool,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<Result<()>> {
        let client = self.client.clone();
        cx.background_executor().spawn(async move {
            let mut action = action;
            if action.edit.is_none() && action.command.is_none() && action.data.is_some() {
                action = serde_json::from_value(client.request("codeAction/resolve", serde_json::to_value(action)?).await?)?;
            }
            if let Some(edit) = action.edit { client.apply_edit(edit).await?; }
            if let Some(command) = action.command {
                client.request("workspace/executeCommand", json!({ "command": command.command, "arguments": command.arguments.unwrap_or_default() })).await?;
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tinymist_snippet_defaults_are_plain_text() {
        assert_eq!(plain_snippet("text(${1:})$0"), "text()");
        assert_eq!(
            plain_snippet("${1:α${2:β}} ${3|red,blue|} \\$5"),
            "αβ red $5"
        );
        assert_eq!(plain_snippet("${1} $0 $literal"), "  $literal");
    }

    #[test]
    fn invalid_variable_creation_is_not_offered_as_a_quick_fix() {
        let uri = "file:///tmp/main.typ";
        let action = |replacement| {
            serde_json::from_value::<CodeAction>(json!({
            "title": "Create missing variable", "edit": { "changes": { uri: [{
                "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 0 } },
                "newText": replacement,
            }] } },
        })).unwrap()
        };
        assert!(!usable_code_action(
            &action("\n\nlet missing"),
            uri,
            "#missing"
        ));
        assert!(usable_code_action(
            &action("#let missing = none\n"),
            uri,
            "#missing"
        ));
    }
}

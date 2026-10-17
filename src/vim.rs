//! A per-buffer modal controller adapted to GPUI Kit's editing API.
//!
//! Zed's `.context/zed/crates/vim/src/{vim,state,motion}.rs` informed the
//! separation of modes, pending operators, counts, and workspace registers.
//! Edits still go through EditorState so LSP and preview receive ordinary changes.
pub mod motion;

use motion as m;
use std::{collections::HashMap, ops::Range};
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Normal,
    Insert,
    Visual,
    VisualLine,
}
impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Normal => "NORMAL",
            Self::Insert => "INSERT",
            Self::Visual => "VISUAL",
            Self::VisualLine => "V-LINE",
        }
    }
    pub fn visual(self) -> bool {
        matches!(self, Self::Visual | Self::VisualLine)
    }
}

#[derive(Clone, Default, Debug)]
pub struct Register {
    pub text: String,
    pub linewise: bool,
}
#[derive(Default)]
pub struct Registers {
    values: HashMap<char, Register>,
    pub clipboard_write: Option<String>,
}
impl Registers {
    pub fn clipboard(&mut self, text: String) {
        self.values.insert(
            '+',
            Register {
                text: text.clone(),
                linewise: text.ends_with('\n'),
            },
        );
        self.values.insert('*', self.values[&'+'].clone());
    }
    fn get(&self, name: Option<char>) -> Register {
        self.values
            .get(&name.unwrap_or('"').to_ascii_lowercase())
            .cloned()
            .unwrap_or_default()
    }
    fn put(&mut self, name: Option<char>, value: Register, yank: bool) {
        if name == Some('_') {
            return;
        }
        if let Some(name) = name {
            if matches!(name, '+' | '*') {
                self.clipboard_write = Some(value.text.clone());
            }
            if name.is_ascii_uppercase() {
                let target = self.values.entry(name.to_ascii_lowercase()).or_default();
                target.text.push_str(&value.text);
                target.linewise |= value.linewise;
            } else {
                self.values.insert(name, value.clone());
            }
        }
        if yank {
            self.values.insert('0', value.clone());
        } else if value.linewise || value.text.contains('\n') {
            for number in ('2'..='9').rev() {
                if let Some(old) = self
                    .values
                    .get(&char::from_u32(number as u32 - 1).unwrap())
                    .cloned()
                {
                    self.values.insert(number, old);
                }
            }
            self.values.insert('1', value.clone());
        } else {
            self.values.insert('-', value.clone());
        }
        self.values.insert('"', value);
    }
}

#[derive(Clone, Debug)]
pub struct Patch {
    pub start: usize,
    pub removed: String,
    pub inserted: String,
}
impl Patch {
    pub fn between(before: &str, after: &str) -> Self {
        let prefix = before
            .graphemes(true)
            .zip(after.graphemes(true))
            .take_while(|(a, b)| a == b)
            .map(|(a, _)| a.len())
            .sum::<usize>();
        let suffix = before[prefix..]
            .graphemes(true)
            .rev()
            .zip(after[prefix..].graphemes(true).rev())
            .take_while(|(a, b)| a == b)
            .map(|(a, _)| a.len())
            .sum::<usize>();
        Self {
            start: prefix,
            removed: before[prefix..before.len() - suffix].into(),
            inserted: after[prefix..after.len() - suffix].into(),
        }
    }
    fn apply(&self, text: &mut String, undo: bool) -> bool {
        let (old, new) = if undo {
            (&self.inserted, &self.removed)
        } else {
            (&self.removed, &self.inserted)
        };
        let range = self.start..self.start + old.len();
        if text.get(range.clone()) != Some(old.as_str()) {
            return false;
        }
        text.replace_range(range, new);
        true
    }
}
#[derive(Clone)]
struct Snapshot {
    text: String,
    cursor: usize,
}
#[derive(Clone)]
struct Change {
    patch: Patch,
    before: usize,
    after: usize,
}
#[derive(Clone)]
struct Repeat {
    keys: Vec<String>,
    insertion: Option<(isize, usize, String)>,
}
#[derive(Clone, Debug)]
enum Pending {
    G,
    Register,
    Find(char),
    Object(bool),
    Replace,
}
#[derive(Clone, Debug)]
pub enum Effect {
    Command(char),
    Definition,
    References,
    Hover,
}
pub struct Outcome {
    pub text: String,
    pub selection: Range<usize>,
    pub effect: Option<Effect>,
}

#[derive(Default)]
pub struct State {
    pub mode: Mode,
    pub message: Option<String>,
    cursor: usize,
    anchor: usize,
    preferred_column: Option<usize>,
    indent_width: Option<usize>,
    count: usize,
    operator: Option<(char, usize)>,
    pending: Option<Pending>,
    register: Option<char>,
    find: Option<(char, char)>,
    search: Option<(String, bool)>,
    observed: Option<Snapshot>,
    before: Option<Snapshot>,
    insertion: Option<Snapshot>,
    keys: Vec<String>,
    repeat: Option<Repeat>,
    undo: Vec<Change>,
    redo: Vec<Change>,
    replaying: bool,
}
impl State {
    pub fn reset(&mut self, text: &str, cursor: usize) {
        *self = Self::default();
        self.cursor = m::clip(text, cursor);
        self.observed = Some(Snapshot {
            text: text.into(),
            cursor: self.cursor,
        });
    }
    pub fn set_indent_width(&mut self, width: usize) {
        self.indent_width = Some(width.clamp(1, 8));
    }
    pub fn needs_clipboard(&self, key: &str) -> bool {
        matches!(key, "p" | "P") && matches!(self.register, Some('+' | '*'))
    }
    pub fn caret(&self) -> usize {
        self.cursor
    }
    pub fn status(&self) -> String {
        let mut label = self.mode.label().to_owned();
        if let Some((op, count)) = self.operator {
            label.push_str(&format!(
                "  {}{}",
                if count > 1 {
                    count.to_string()
                } else {
                    String::new()
                },
                op
            ));
        }
        if self.count > 0 {
            label.push_str(&format!(" {}", self.count));
        }
        if self.pending.is_some() {
            label.push_str(" …");
        }
        label
    }
    pub fn observe(&mut self, text: &str, cursor: usize) {
        if let Some(old) = self.observed.take()
            && old.text != text
            && self.mode != Mode::Insert
        {
            self.record(&old, text, cursor);
            // Formatting/rename can replace a Visual selection and shorten the
            // buffer. Cancel pending commands anchored to the old document.
            self.mode = Mode::Normal;
            self.cursor = m::clip(text, cursor);
            self.anchor = self.cursor;
            self.preferred_column = None;
            self.before = None;
            self.keys.clear();
            self.clear();
        }
        self.observed = Some(Snapshot {
            text: text.into(),
            cursor,
        });
    }
    fn record(&mut self, before: &Snapshot, text: &str, cursor: usize) {
        if before.text == text || self.replaying {
            return;
        }
        self.undo.push(Change {
            patch: Patch::between(&before.text, text),
            before: before.cursor,
            after: cursor,
        });
        if self.undo.len() > 200 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }
    pub fn selection(&self, text: &str) -> Range<usize> {
        match self.mode {
            Mode::Visual if self.cursor < self.anchor => Range {
                start: m::next(text, self.anchor),
                end: self.cursor,
            },
            Mode::Visual => self.anchor..m::next(text, self.cursor),
            Mode::VisualLine if self.cursor < self.anchor => Range {
                start: m::after_line(text, self.anchor),
                end: m::start(text, self.cursor),
            },
            Mode::VisualLine => m::start(text, self.anchor)..m::after_line(text, self.cursor),
            _ => self.cursor..self.cursor,
        }
    }
    fn selected(&self, text: &str) -> Range<usize> {
        let range = self.selection(text);
        range.start.min(range.end)..range.start.max(range.end)
    }
    fn clear(&mut self) {
        self.operator = None;
        self.pending = None;
        self.count = 0;
        self.register = None;
    }
    fn insert(&mut self, text: &str) {
        self.preferred_column = None;
        self.mode = Mode::Insert;
        self.insertion = Some(Snapshot {
            text: text.into(),
            cursor: self.cursor,
        });
    }
    fn move_to(&mut self, text: &str, to: usize, vertical: bool) {
        self.cursor = m::clip(text, to);
        if !vertical {
            self.preferred_column = None;
        }
    }
    fn operate(
        &mut self,
        text: &mut String,
        range: Range<usize>,
        linewise: bool,
        op: char,
        registers: &mut Registers,
    ) {
        let mut range = range.start.min(text.len())..range.end.min(text.len());
        if linewise {
            // Visual S/C can start in the middle of a line. Expand the range
            // before reading indentation; first nonblank may precede the
            // selection, which would otherwise produce a reversed slice.
            range.start = m::start(text, range.start);
            if range.end > range.start && !text[..range.end].ends_with('\n') {
                range.end = m::after_line(text, range.end);
            }
        }
        if range.is_empty() && op != 'c' {
            return;
        }
        if matches!(op, 'd' | 'c' | 'y') {
            let mut value = text[range.clone()].to_owned();
            if linewise && !value.ends_with('\n') {
                value.push('\n');
            }
            registers.put(
                self.register,
                Register {
                    text: value,
                    linewise,
                },
                op == 'y',
            );
        }
        let mut replacement = String::new();
        match op {
            'y' => {
                self.cursor = range.start;
                self.mode = Mode::Normal;
                return;
            }
            '>' | '<' => {
                let source = &text[range.clone()];
                let width = self.indent_width.unwrap_or(2);
                let indent = " ".repeat(width);
                replacement = source
                    .split_inclusive('\n')
                    .map(|line| {
                        if op == '>' {
                            format!("{indent}{line}")
                        } else {
                            let spaces =
                                line.chars().take(width).take_while(|ch| *ch == ' ').count();
                            if spaces > 0 {
                                line[spaces..].to_owned()
                            } else {
                                line.strip_prefix('\t').unwrap_or(line).to_owned()
                            }
                        }
                    })
                    .collect();
            }
            'U' => replacement = text[range.clone()].to_uppercase(),
            'u' => replacement = text[range.clone()].to_lowercase(),
            '~' => {
                replacement = text[range.clone()]
                    .chars()
                    .flat_map(|ch| {
                        if ch.is_lowercase() {
                            ch.to_uppercase().collect::<Vec<_>>()
                        } else {
                            ch.to_lowercase().collect()
                        }
                    })
                    .collect()
            }
            'c' if linewise => {
                let indent = &text[range.start..m::first(text, range.start)];
                replacement = indent.to_owned();
                if text[range.clone()].ends_with('\n') {
                    replacement.push('\n');
                }
            }
            'd' if linewise
                && range.end == text.len()
                && !text.ends_with('\n')
                && range.start > 0 =>
            {
                range.start = m::previous(text, range.start);
            }
            _ => {}
        }
        text.replace_range(range.clone(), &replacement);
        self.cursor = range.start;
        self.mode = Mode::Normal;
        if op == 'c' {
            if linewise {
                self.cursor = m::first(text, self.cursor);
            }
            self.insert(text);
        } else {
            self.cursor = m::clip(text, self.cursor);
            if linewise {
                self.cursor = m::clip(text, m::first(text, self.cursor));
            }
        }
    }
    fn motion(&mut self, text: &str, key: &str, count: usize) -> Option<(usize, bool, bool)> {
        let mut at = self.cursor;
        let mut inclusive = false;
        let mut linewise = false;
        match key {
            "h" | "left" | "backspace" => {
                for _ in 0..count {
                    if at > m::start(text, at) {
                        at = m::previous(text, at);
                    }
                }
            }
            "l" | "right" | " " => {
                for _ in 0..count {
                    if at < m::end(text, at) {
                        at = m::next(text, at);
                    }
                }
            }
            "j" | "down" | "k" | "up" | "ctrl-d" | "ctrl-u" | "ctrl-f" | "ctrl-b" => {
                let column = *self
                    .preferred_column
                    .get_or_insert_with(|| m::column(text, at));
                let direction = if matches!(key, "k" | "up" | "ctrl-u" | "ctrl-b") {
                    -1
                } else {
                    1
                };
                let amount = count
                    * if key.starts_with("ctrl-") {
                        if matches!(key, "ctrl-f" | "ctrl-b") {
                            30
                        } else {
                            15
                        }
                    } else {
                        1
                    };
                let row = m::row(text, at).saturating_add_signed(direction * amount as isize);
                at = m::at_column(text, m::line_number(text, row), column);
                linewise = true;
            }
            "w" | "b" | "e" | "W" | "B" | "E" => {
                let key = key.chars().next().unwrap();
                at = m::word(text, at, key, count);
                inclusive = matches!(key, 'e' | 'E');
            }
            "enter" | "\n" | "+" | "-" => {
                let row = m::row(text, at).saturating_add_signed(if key == "-" {
                    -(count as isize)
                } else {
                    count as isize
                });
                at = m::first(text, m::line_number(text, row));
                linewise = true;
            }
            "0" | "home" => at = m::start(text, at),
            "^" | "_" => at = m::first(text, at),
            "$" | "end" => {
                at = m::end(text, m::line_number(text, m::row(text, at) + count - 1));
            }
            "G" => {
                at = m::first(
                    text,
                    m::line_number(
                        text,
                        if self.count > 0 || self.operator.is_some() {
                            count - 1
                        } else {
                            usize::MAX
                        },
                    ),
                );
                linewise = true;
            }
            "%" => {
                at = m::matching_pair(text, at)?;
                inclusive = true;
            }
            "{" | "}" => {
                for _ in 0..count {
                    let mut line = m::row(text, at);
                    let forward = key == "}";
                    let mut moved = false;
                    loop {
                        let next = if forward {
                            line.saturating_add(1)
                        } else {
                            line.saturating_sub(1)
                        };
                        if next == line {
                            break;
                        }
                        let offset = m::line_number(text, next);
                        if m::row(text, offset) != next {
                            at = offset;
                            break;
                        }
                        line = next;
                        at = offset;
                        if text[offset..m::end(text, offset)].trim().is_empty() {
                            if moved {
                                break;
                            }
                        } else {
                            moved = true;
                        }
                    }
                }
            }
            _ => return None,
        }
        Some((at, inclusive, linewise))
    }
    fn apply_motion(
        &mut self,
        text: &mut String,
        to: usize,
        inclusive: bool,
        linewise: bool,
        registers: &mut Registers,
    ) {
        if let Some((op, _)) = self.operator.take() {
            let range = if linewise {
                m::start(text, self.cursor.min(to))..m::after_line(text, self.cursor.max(to))
            } else if inclusive {
                self.cursor.min(to)..m::next(text, self.cursor.max(to))
            } else {
                self.cursor.min(to)..self.cursor.max(to)
            };
            self.operate(text, range, linewise, op, registers);
        } else {
            self.move_to(text, to, linewise);
        }
        self.count = 0;
    }
    pub fn search(
        &mut self,
        text: &str,
        pattern: &str,
        forward: bool,
    ) -> Result<Range<usize>, String> {
        let pattern = if pattern.is_empty() {
            self.search
                .as_ref()
                .map(|(p, _)| p.as_str())
                .ok_or("No previous search")?
        } else {
            pattern
        };
        let regex = regex::Regex::new(pattern).map_err(|e| e.to_string())?;
        let found = if forward {
            regex
                .find_iter(text)
                .find(|found| found.start() > self.cursor)
                .or_else(|| regex.find(text))
        } else {
            regex
                .find_iter(text)
                .filter(|found| found.start() < self.cursor)
                .last()
                .or_else(|| regex.find_iter(text).last())
        };
        let Some(found) = found else {
            return Err(format!("Pattern not found: {pattern}"));
        };
        self.cursor = m::clip(text, found.start());
        self.search = Some((pattern.into(), forward));
        self.clear();
        self.before = None;
        self.keys.clear();
        Ok(self.selection(text))
    }
    pub fn goto_line(&mut self, text: &str, line: usize) -> Range<usize> {
        if self.mode != Mode::Insert {
            self.mode = Mode::Normal;
        }
        self.cursor = m::clip(
            text,
            m::first(text, m::line_number(text, line.saturating_sub(1))),
        );
        self.clear();
        self.before = None;
        self.keys.clear();
        self.selection(text)
    }
    pub fn key(
        &mut self,
        key: &str,
        text: &str,
        cursor: usize,
        selection: Range<usize>,
        registers: &mut Registers,
    ) -> Option<Outcome> {
        if self.mode == Mode::Insert && !matches!(key, "escape" | "ctrl-[" | "ctrl-c") {
            return None;
        }
        if key.starts_with("ctrl-")
            && !matches!(
                key,
                "ctrl-["
                    | "ctrl-c"
                    | "ctrl-r"
                    | "ctrl-z"
                    | "ctrl-shift-z"
                    | "ctrl-d"
                    | "ctrl-u"
                    | "ctrl-f"
                    | "ctrl-b"
            )
        {
            return None;
        }
        if key.starts_with("alt-")
            || key.starts_with("cmd-")
            || (key.starts_with('f')
                && key.len() > 1
                && key[1..].chars().all(|ch| ch.is_ascii_digit()))
        {
            return None;
        }
        self.observe(text, cursor);
        if self.mode != Mode::Insert {
            // Mouse selections become Visual selections; otherwise honor a native click.
            if self.mode.visual() && selection.is_empty() {
                self.mode = Mode::Normal;
            }
            if !selection.is_empty() && !self.mode.visual() {
                self.mode = Mode::Visual;
                self.anchor = if cursor == selection.start {
                    m::previous(text, selection.end)
                } else {
                    selection.start
                };
            }
            if !self.mode.visual() {
                self.cursor = m::clip(text, cursor);
            } else if !selection.is_empty() && selection != self.selected(text) {
                self.anchor = if cursor == selection.start {
                    m::previous(text, selection.end)
                } else {
                    selection.start
                };
                self.cursor = if cursor == selection.end {
                    m::previous(text, cursor)
                } else {
                    cursor
                };
            }
        } else {
            self.cursor = cursor;
        }
        self.message = None;
        if self.before.is_none() {
            self.before = Some(Snapshot {
                text: text.into(),
                cursor: self.cursor,
            });
            self.keys.clear();
        }
        self.keys.push(key.into());
        let selecting_register = matches!(self.pending, Some(Pending::Register));
        let mut output = text.to_owned();
        let mut effect = None;
        let mut history = false;
        if matches!(key, "escape" | "ctrl-[" | "ctrl-c") {
            if self.mode == Mode::Insert {
                if let Some(base) = self.insertion.take() {
                    let patch = Patch::between(&base.text, text);
                    let delta = if patch.start >= base.cursor {
                        base.text[base.cursor..patch.start].graphemes(true).count() as isize
                    } else {
                        -(base.text[patch.start..base.cursor].graphemes(true).count() as isize)
                    };
                    if !self.replaying
                        && self
                            .before
                            .as_ref()
                            .is_some_and(|before| before.text != text)
                    {
                        self.repeat = Some(Repeat {
                            keys: self.keys[..self.keys.len() - 1].to_vec(),
                            insertion: Some((
                                delta,
                                patch.removed.graphemes(true).count(),
                                patch.inserted,
                            )),
                        });
                    }
                }
                if self.cursor > m::start(text, self.cursor) {
                    self.cursor = m::previous(text, self.cursor);
                }
            }
            self.mode = Mode::Normal;
            self.clear();
        } else if matches!(key, "u" | "ctrl-z" | "ctrl-r" | "ctrl-shift-z")
            && !self.mode.visual()
            && self.operator.is_none()
            && self.pending.is_none()
        {
            let redo = matches!(key, "ctrl-r" | "ctrl-shift-z");
            for _ in 0..self.count.max(1) {
                let change = if redo {
                    self.redo.pop()
                } else {
                    self.undo.pop()
                };
                if let Some(change) = change {
                    if change.patch.apply(&mut output, !redo) {
                        self.cursor = if redo { change.after } else { change.before };
                        if redo {
                            self.undo.push(change);
                        } else {
                            self.redo.push(change);
                        }
                    } else {
                        self.undo.clear();
                        self.redo.clear();
                        self.message = Some("Undo history invalidated by an external edit".into());
                        break;
                    }
                }
            }
            self.clear();
            history = true;
        } else if key == "."
            && self.pending.is_none()
            && self.operator.is_none()
            && !self.mode.visual()
        {
            let before = self.before.take().unwrap();
            let count = self.count.max(1);
            self.clear();
            if let Some(repeat) = self.repeat.clone() {
                self.replaying = true;
                for _ in 0..count {
                    for key in &repeat.keys {
                        let selected = self.selection(&output);
                        let normalized =
                            selected.start.min(selected.end)..selected.start.max(selected.end);
                        if let Some(result) =
                            self.key(key, &output, self.cursor, normalized, registers)
                        {
                            output = result.text;
                        }
                    }
                    if let Some((delta, removed, inserted)) = &repeat.insertion {
                        let mut begin = self.cursor;
                        for _ in 0..delta.unsigned_abs() {
                            begin = if *delta < 0 {
                                m::previous(&output, begin)
                            } else {
                                m::next(&output, begin)
                            };
                        }
                        let mut end = begin;
                        for _ in 0..*removed {
                            end = m::next(&output, end);
                        }
                        output.replace_range(begin..end, inserted);
                        self.cursor = begin + inserted.len();
                        self.mode = Mode::Normal;
                        self.insertion = None;
                        if self.cursor > m::start(&output, self.cursor) {
                            self.cursor = m::previous(&output, self.cursor);
                        }
                    }
                    self.before = None;
                    self.clear();
                }
                self.replaying = false;
                self.record(&before, &output, self.cursor);
            }
            self.before = None;
            self.keys.clear();
            history = true;
        } else if let Some(pending) = self.pending.take() {
            let count = self
                .count
                .max(1)
                .saturating_mul(self.operator.map_or(1, |(_, count)| count))
                .min(100_000);
            match pending {
                Pending::Register => {
                    self.register = key.chars().next();
                }
                Pending::Replace => {
                    if key.chars().count() == 1 {
                        let mut end = self.cursor;
                        for _ in 0..count {
                            if end < m::end(&output, end) {
                                end = m::next(&output, end);
                            }
                        }
                        let replacement =
                            key.repeat(output[self.cursor..end].graphemes(true).count());
                        output.replace_range(self.cursor..end, &replacement);
                        self.cursor = m::clip(
                            &output,
                            self.cursor + replacement.len().saturating_sub(key.len()),
                        );
                    }
                    self.clear();
                }
                Pending::Object(around) => {
                    let object = key.chars().next().unwrap_or('\0');
                    let range = match object {
                        'w' | 'W' => Some(m::word_object(
                            &output,
                            self.cursor,
                            around,
                            object == 'W',
                            count,
                        )),
                        _ => m::pair_object(&output, self.cursor, object, around, count),
                    };
                    if let Some(range) = range {
                        if let Some((op, _)) = self.operator.take() {
                            self.operate(&mut output, range, false, op, registers);
                        } else {
                            self.mode = Mode::Visual;
                            self.anchor = range.start;
                            self.cursor = m::previous(&output, range.end);
                        }
                    }
                    self.count = 0;
                }
                Pending::Find(direction) => {
                    if let Some(target) = key.chars().next() {
                        self.find = Some((direction, target));
                        self.find_motion(&mut output, direction, target, count, false, registers);
                    }
                    self.count = 0;
                }
                Pending::G => match key {
                    "g" => {
                        let to = m::first(&output, m::line_number(&output, count - 1));
                        self.apply_motion(&mut output, to, false, true, registers);
                    }
                    "d" => {
                        effect = Some(Effect::Definition);
                        self.clear();
                    }
                    "r" => {
                        effect = Some(Effect::References);
                        self.clear();
                    }
                    "U" | "u" => {
                        let op = key.chars().next().unwrap();
                        if self.mode.visual() {
                            let range = self.selected(&output);
                            self.operate(
                                &mut output,
                                range,
                                self.mode == Mode::VisualLine,
                                op,
                                registers,
                            );
                            self.clear();
                        } else {
                            self.operator = Some((op, count));
                            self.count = 0;
                        }
                    }
                    _ => self.clear(),
                },
            }
        } else if key.chars().count() == 1
            && key.as_bytes()[0].is_ascii_digit()
            && (key != "0" || self.count > 0)
        {
            self.count = self
                .count
                .saturating_mul(10)
                .saturating_add((key.as_bytes()[0] - b'0') as usize)
                .min(100_000);
        } else {
            let count = self
                .count
                .max(1)
                .saturating_mul(self.operator.map_or(1, |(_, count)| count))
                .min(100_000);
            let visual = self.mode.visual();
            match key {
                "\"" => self.pending = Some(Pending::Register),
                "g" => self.pending = Some(Pending::G),
                "f" | "F" | "t" | "T" => {
                    self.pending = Some(Pending::Find(key.chars().next().unwrap()))
                }
                ";" | "," => {
                    if let Some((mut direction, target)) = self.find {
                        if key == "," {
                            direction = if direction.is_uppercase() {
                                direction.to_ascii_lowercase()
                            } else {
                                direction.to_ascii_uppercase()
                            };
                        }
                        self.find_motion(&mut output, direction, target, count, true, registers);
                    }
                    self.count = 0;
                }
                "i" | "a" if self.operator.is_some() || visual => {
                    self.pending = Some(Pending::Object(key == "a"))
                }
                "d" | "c" | "y" | ">" | "<" => {
                    let op = key.chars().next().unwrap();
                    if visual {
                        let range = self.selected(&output);
                        let range = if matches!(op, '>' | '<') {
                            m::start(&output, range.start)
                                ..m::after_line(&output, m::previous(&output, range.end))
                        } else {
                            range
                        };
                        self.operate(
                            &mut output,
                            range,
                            self.mode == Mode::VisualLine,
                            op,
                            registers,
                        );
                        self.clear();
                    } else if self.operator.is_some_and(|(previous, _)| previous == op) {
                        let range = m::line_range(&output, self.cursor, count);
                        self.operate(&mut output, range, true, op, registers);
                        self.clear();
                    } else {
                        self.operator = Some((op, self.count.max(1)));
                        self.count = 0;
                    }
                }
                "u" | "U"
                    if visual
                        || self
                            .operator
                            .is_some_and(|(op, _)| op == key.chars().next().unwrap()) =>
                {
                    let range = if visual {
                        self.selected(&output)
                    } else {
                        m::line_range(&output, self.cursor, count)
                    };
                    self.operate(
                        &mut output,
                        range,
                        self.mode == Mode::VisualLine || !visual,
                        key.chars().next().unwrap(),
                        registers,
                    );
                    self.clear();
                }
                "v" | "V" => {
                    let mode = if key == "v" {
                        Mode::Visual
                    } else {
                        Mode::VisualLine
                    };
                    if self.mode == mode {
                        self.mode = Mode::Normal;
                    } else {
                        if !visual {
                            self.anchor = self.cursor;
                        }
                        self.mode = mode;
                    }
                    self.clear();
                }
                "i" | "a" | "I" | "A" | "o" | "O" => {
                    match key {
                        "a" => {
                            if self.cursor < m::end(&output, self.cursor) {
                                self.cursor = m::next(&output, self.cursor);
                            }
                        }
                        "I" => self.cursor = m::first(&output, self.cursor),
                        "A" => self.cursor = m::end(&output, self.cursor),
                        "o" | "O" => {
                            let indent = output
                                [m::start(&output, self.cursor)..m::first(&output, self.cursor)]
                                .to_owned();
                            if key == "O" {
                                self.cursor = m::start(&output, self.cursor);
                                output.insert_str(self.cursor, &format!("{indent}\n"));
                                self.cursor += indent.len();
                            } else {
                                self.cursor = m::end(&output, self.cursor);
                                output.insert_str(self.cursor, &format!("\n{indent}"));
                                self.cursor += 1 + indent.len();
                            }
                        }
                        _ => {}
                    }
                    self.insert(&output);
                    self.clear();
                }
                "D" | "C" | "Y" | "x" | "delete" | "X" | "s" | "S" => {
                    let linewise = key == "Y" || key == "S" || self.mode == Mode::VisualLine;
                    let range = if visual {
                        self.selected(&output)
                    } else {
                        match key {
                            "Y" | "S" => m::line_range(&output, self.cursor, count),
                            "D" | "C" => {
                                self.cursor
                                    ..m::end(
                                        &output,
                                        m::line_number(
                                            &output,
                                            m::row(&output, self.cursor) + count - 1,
                                        ),
                                    )
                            }
                            "X" => {
                                let mut begin = self.cursor;
                                for _ in 0..count {
                                    if begin > m::start(&output, begin) {
                                        begin = m::previous(&output, begin);
                                    }
                                }
                                begin..self.cursor
                            }
                            _ => {
                                let mut end = self.cursor;
                                for _ in 0..count {
                                    if end < m::end(&output, end) {
                                        end = m::next(&output, end);
                                    }
                                }
                                self.cursor..end
                            }
                        }
                    };
                    self.operate(
                        &mut output,
                        range,
                        linewise,
                        if key == "Y" {
                            'y'
                        } else if matches!(key, "C" | "s" | "S") {
                            'c'
                        } else {
                            'd'
                        },
                        registers,
                    );
                    self.clear();
                }
                "p" | "P" => {
                    let value = registers.get(self.register);
                    if !value.text.is_empty() {
                        let mut paste = value.text.repeat(count);
                        let at = if visual {
                            let range = self.selected(&output);
                            let linewise = self.mode == Mode::VisualLine;
                            let deleted = Register {
                                text: output[range.clone()].into(),
                                linewise,
                            };
                            registers.put(None, deleted, false);
                            if linewise
                                && !paste.ends_with('\n')
                                && output[range.clone()].ends_with('\n')
                            {
                                paste.push('\n');
                            }
                            output.replace_range(range.clone(), &paste);
                            range.start
                        } else {
                            let at = if value.linewise {
                                if key == "P" {
                                    m::start(&output, self.cursor)
                                } else {
                                    let end = m::after_line(&output, self.cursor);
                                    if end == output.len()
                                        && !output.is_empty()
                                        && !output.ends_with('\n')
                                    {
                                        paste.insert(0, '\n');
                                    }
                                    end
                                }
                            } else if key == "p" && self.cursor < m::end(&output, self.cursor) {
                                m::next(&output, self.cursor)
                            } else {
                                self.cursor
                            };
                            output.insert_str(at, &paste);
                            at
                        };
                        self.cursor = if value.linewise {
                            m::first(&output, at + usize::from(paste.starts_with('\n')))
                        } else {
                            m::previous(&output, at + paste.len())
                        };
                        self.mode = Mode::Normal;
                    }
                    self.clear();
                }
                "r" => {
                    if visual {
                        self.message = Some("Use c to replace a Visual selection".into());
                        self.clear();
                    } else {
                        self.pending = Some(Pending::Replace);
                    }
                }
                "~" => {
                    let range = if visual {
                        self.selected(&output)
                    } else {
                        let mut end = self.cursor;
                        for _ in 0..count {
                            if end < m::end(&output, end) {
                                end = m::next(&output, end);
                            }
                        }
                        self.cursor..end
                    };
                    self.operate(
                        &mut output,
                        range,
                        self.mode == Mode::VisualLine,
                        '~',
                        registers,
                    );
                    self.clear();
                }
                "J" => {
                    let mut range = if visual {
                        self.selected(&output)
                    } else {
                        m::line_range(&output, self.cursor, count.max(2))
                    };
                    range.start = m::start(&output, range.start);
                    let last = m::previous(&output, range.end);
                    range.end = m::end(&output, last);
                    let old = &output[range.clone()];
                    let joined = old
                        .lines()
                        .enumerate()
                        .map(|(i, line)| if i == 0 { line.trim_end() } else { line.trim() })
                        .collect::<Vec<_>>()
                        .join(" ");
                    let at = m::end(&output, self.cursor);
                    output.replace_range(range, &joined);
                    self.cursor = m::clip(&output, at);
                    self.mode = Mode::Normal;
                    self.clear();
                }
                ":" | "/" | "?" => {
                    effect = Some(Effect::Command(key.chars().next().unwrap()));
                    self.clear();
                }
                "n" | "N" => {
                    if let Some((pattern, forward)) = self.search.clone() {
                        for _ in 0..count {
                            if let Err(error) = self.search(
                                &output,
                                &pattern,
                                if key == "n" { forward } else { !forward },
                            ) {
                                self.message = Some(error);
                            }
                        }
                        self.search = Some((pattern, forward));
                    }
                    self.clear();
                }
                "*" | "#" => {
                    let range = m::word_object(&output, self.cursor, false, false, 1);
                    let word = &output[range];
                    let pattern = format!("\\b{}\\b", regex::escape(word));
                    if let Err(error) = self.search(&output, &pattern, key == "*") {
                        self.message = Some(error);
                    }
                    self.clear();
                }
                "K" => {
                    effect = Some(Effect::Hover);
                    self.clear();
                }
                _ => {
                    let mut motion_key = key;
                    if self.operator.is_some_and(|(op, _)| op == 'c')
                        && matches!(key, "w" | "W")
                        && !output[self.cursor..].starts_with(char::is_whitespace)
                    {
                        motion_key = if key == "w" { "e" } else { "E" };
                    }
                    if let Some((mut to, inclusive, linewise)) =
                        self.motion(&output, motion_key, count)
                    {
                        if matches!(motion_key, "w" | "W")
                            && self.operator.is_some()
                            && m::row(&output, to) > m::row(&output, self.cursor)
                            && m::end(&output, self.cursor) > self.cursor
                        {
                            to = m::end(&output, self.cursor);
                        }
                        self.apply_motion(&mut output, to, inclusive, linewise, registers);
                    } else {
                        self.clear();
                    }
                }
            }
        }
        if output != text || matches!(key, "escape" | "ctrl-[" | "ctrl-c") {
            self.preferred_column = None;
        }
        if self.mode != Mode::Insert {
            self.cursor = m::clip(&output, self.cursor);
        }
        let finished = !selecting_register
            && self.operator.is_none()
            && self.pending.is_none()
            && self.count == 0
            && self.mode != Mode::Insert;
        if finished {
            if let Some(before) = self.before.take()
                && !history
            {
                if before.text != output
                    && !self.replaying
                    && self.insertion.is_none()
                    && !matches!(key, "escape" | "ctrl-[" | "ctrl-c")
                {
                    self.repeat = Some(Repeat {
                        keys: self.keys.clone(),
                        insertion: None,
                    });
                }
                self.record(&before, &output, self.cursor);
            }
            self.keys.clear();
            self.register = None;
        }
        self.observed = Some(Snapshot {
            text: output.clone(),
            cursor: self.cursor,
        });
        Some(Outcome {
            selection: self.selection(&output),
            text: output,
            effect,
        })
    }
    fn find_motion(
        &mut self,
        text: &mut String,
        direction: char,
        target: char,
        count: usize,
        repeated: bool,
        registers: &mut Registers,
    ) {
        let forward = direction.is_lowercase();
        let end = m::end(text, self.cursor);
        let mut start = m::next(text, self.cursor).min(end);
        if repeated && direction == 't' {
            start = m::next(text, start).min(end);
        }
        let backward_end = if repeated && direction == 'T' {
            m::previous(text, self.cursor).max(m::start(text, self.cursor))
        } else {
            self.cursor
        };
        let found = if forward {
            text[start..end]
                .char_indices()
                .filter(|(_, ch)| *ch == target)
                .nth(count - 1)
                .map(|(offset, _)| start + offset)
        } else {
            text[m::start(text, self.cursor)..backward_end]
                .char_indices()
                .rev()
                .filter(|(_, ch)| *ch == target)
                .nth(count - 1)
                .map(|(offset, _)| m::start(text, self.cursor) + offset)
        };
        if let Some(mut to) = found {
            if direction == 't' {
                to = m::previous(text, to);
            }
            if direction == 'T' {
                to = m::next(text, to);
            }
            self.apply_motion(text, to, true, false, registers);
        } else {
            self.operator = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Editor {
        state: State,
        registers: Registers,
        text: String,
        cursor: usize,
        selection: Range<usize>,
    }
    impl Editor {
        fn new(text: &str) -> Self {
            let mut state = State::default();
            state.reset(text, 0);
            Self {
                state,
                registers: Registers::default(),
                text: text.into(),
                cursor: 0,
                selection: 0..0,
            }
        }
        fn keys(&mut self, keys: &[&str]) {
            for key in keys {
                let result = self
                    .state
                    .key(
                        key,
                        &self.text,
                        self.cursor,
                        self.selection.clone(),
                        &mut self.registers,
                    )
                    .unwrap();
                self.text = result.text;
                self.cursor = self.state.cursor;
                let selection = result.selection;
                self.selection =
                    selection.start.min(selection.end)..selection.start.max(selection.end);
            }
        }
        fn type_text(&mut self, text: &str) {
            assert_eq!(self.state.mode, Mode::Insert);
            self.text.insert_str(self.cursor, text);
            self.cursor += text.len();
            self.selection = self.cursor..self.cursor;
            self.state.observe(&self.text, self.cursor);
        }
    }
    #[test]
    fn visual_line_change_from_midline_preserves_indent_and_undo() {
        let mut simple = Editor::new("one é 😀\n");
        simple.keys(&["w", "v", "S"]);
        assert_eq!(simple.state.mode, Mode::Insert);
        assert_eq!(simple.text, "\n");
        let mut editor = Editor::new("first\n  second line\nthird\n");
        editor.keys(&["j", "w", "v", "j", "S"]);
        assert_eq!(editor.state.mode, Mode::Insert);
        assert_eq!(editor.text, "first\n  \n");
        editor.type_text("replacement");
        editor.keys(&["escape", "u"]);
        assert_eq!(editor.text, "first\n  second line\nthird\n");
    }

    #[test]
    fn operators_counts_registers_and_linewise_paste() {
        let mut editor = Editor::new("one two three four five six seven\nlast");
        editor.keys(&["2", "d", "3", "w"]);
        assert_eq!(editor.text, "seven\nlast");
        editor.keys(&["u"]);
        assert!(editor.text.starts_with("one two"));
        editor.keys(&["\"", "a", "y", "y", "G", "\"", "a", "p"]);
        assert_eq!(
            editor.text,
            "one two three four five six seven\nlast\none two three four five six seven\n"
        );
        editor.keys(&["u", "g", "g", "\"", "_", "d", "w"]);
        assert_eq!(editor.text, "two three four five six seven\nlast");
        assert_eq!(
            editor.registers.get(Some('a')).text,
            "one two three four five six seven\n"
        );
        editor.keys(&["u", "ctrl-r"]);
        assert_eq!(editor.text, "two three four five six seven\nlast");
        let mut editor = Editor::new("alpha\n  beta");
        editor.state.set_indent_width(4);
        editor.keys(&[">", ">"]);
        assert_eq!(editor.text, "    alpha\n  beta");
        editor.keys(&["<", "<", "j", "<", "<"]);
        assert_eq!(editor.text, "alpha\nbeta");
    }
    #[test]
    fn change_insert_is_one_undo_and_dot_replays_semantics() {
        let mut editor = Editor::new("one elephant three");
        editor.keys(&["c", "i", "w"]);
        editor.type_text("small");
        editor.keys(&["escape"]);
        assert_eq!(editor.text, "small elephant three");
        editor.keys(&["w", "."]);
        assert_eq!(editor.text, "small small three");
        editor.keys(&["u"]);
        assert_eq!(editor.text, "small elephant three");
        editor.keys(&["u"]);
        assert_eq!(editor.text, "one elephant three");
        editor.keys(&["ctrl-r", "ctrl-r"]);
        assert_eq!(editor.text, "small small three");
        editor.keys(&["A"]);
        editor.type_text("!");
        editor.keys(&["escape", "g", "g", "."]);
        assert_eq!(editor.text, "small small three!!");
    }
    #[test]
    fn visual_backwards_objects_and_unicode() {
        let mut editor = Editor::new("a\u{301}😀 rust (one (two) three)\nlast");
        editor.keys(&["l", "x"]);
        assert_eq!(editor.text, "a\u{301} rust (one (two) three)\nlast");
        editor.keys(&["u", "l", "v", "h", "d"]);
        assert_eq!(editor.text, "a\u{301}rust (one (two) three)\nlast");
        editor.keys(&["u", "f", "(", "l", "f", "(", "l", "c", "i", "("]);
        editor.type_text("new");
        editor.keys(&["escape"]);
        assert!(editor.text.contains("(one (new) three)"));
        editor.keys(&["u"]);
        assert!(editor.text.contains("(one (two) three)"));
        editor.keys(&["g", "g", "V", "j", "y"]);
        assert!(editor.registers.get(None).linewise);
        assert!(editor.registers.get(None).text.ends_with("last\n"));
    }
    #[test]
    fn find_search_motion_columns_and_external_edits() {
        let mut editor = Editor::new("longer line\nx\nlonger line\n");
        editor.keys(&["4", "l", "j", "j"]);
        assert_eq!(m::column(&editor.text, editor.cursor), 4);
        editor.keys(&["g", "g", "d", "f", "r"]);
        assert_eq!(editor.text, " line\nx\nlonger line\n");
        editor.keys(&["u"]);
        let selected = editor.state.search(&editor.text, "line", true).unwrap();
        editor.cursor = selected.start;
        editor.selection = selected;
        assert_eq!(editor.cursor, 7);
        editor.keys(&["n"]);
        assert_eq!(editor.cursor, 21);
        editor.keys(&["N"]);
        assert_eq!(editor.cursor, 7);
        editor.text.insert_str(0, "# ");
        editor.state.observe(&editor.text, editor.cursor);
        editor.keys(&["u"]);
        assert!(editor.text.starts_with("longer"));
    }
    #[test]
    fn opening_lines_indent_and_change_last_line() {
        let mut editor = Editor::new("  first\n  last");
        editor.keys(&["o"]);
        editor.type_text("middle");
        editor.keys(&["escape"]);
        assert_eq!(editor.text, "  first\n  middle\n  last");
        editor.keys(&["u", "G", "c", "c"]);
        editor.type_text("new");
        editor.keys(&["escape"]);
        assert_eq!(editor.text, "  first\n  new");
        editor.keys(&["u", "G", "d", "d"]);
        assert_eq!(editor.text, "  first");
        editor.keys(&["u", "g", "g", ">", ">"]);
        assert_eq!(editor.text, "    first\n  last");
    }
    #[test]
    fn line_endings_empty_buffers_and_case_operators() {
        let mut editor = Editor::new("first\nlast\n");
        editor.keys(&["G"]);
        assert_eq!(editor.cursor, 6);
        editor.keys(&["d", "d"]);
        assert_eq!(editor.text, "first\n");
        assert_eq!(editor.cursor, 0);
        editor.keys(&["d", "d", "p"]);
        assert_eq!(editor.text, "first\n");
        editor.keys(&["g", "U", "U"]);
        assert_eq!(editor.text, "FIRST\n");
        editor.keys(&["v", "l", "g", "u"]);
        assert_eq!(editor.text, "fiRST\n");
        let mut editor = Editor::new("\nlast");
        editor.keys(&["a"]);
        editor.type_text("new");
        editor.keys(&["escape"]);
        assert_eq!(editor.text, "new\nlast");
        let mut editor = Editor::new("word\r\nnext\r\n");
        editor.keys(&["G", "d", "d"]);
        assert_eq!(editor.text, "word\r\n");
        assert_eq!(m::clip("a\u{301}b", 1), 0);
        let mut editor = Editor::new("\nnext");
        editor.keys(&["f", "x", "t", "x"]);
        assert_eq!(editor.cursor, 0);
        let mut editor = Editor::new("a,b,c,d");
        editor.keys(&["t", ",", ";"]);
        assert_eq!(editor.cursor, 2);
        editor.keys(&[";"]);
        assert_eq!(editor.cursor, 4);
        let mut editor = Editor::new("one two");
        editor.keys(&["x", "i", "escape", "."]);
        assert_eq!(editor.text, "e two");
    }
    #[test]
    fn visual_paste_replaces_at_original_position_including_end_of_line() {
        let mut editor = Editor::new("new old");
        editor.keys(&["y", "i", "w", "w", "v", "e", "p"]);
        assert_eq!(editor.text, "new new");
        editor.keys(&["u"]);
        assert_eq!(editor.text, "new old");
        let mut editor = Editor::new("one\ntwo\nthree\n");
        editor.keys(&["y", "y", "j", "V", "p"]);
        assert_eq!(editor.text, "one\none\nthree\n");
    }
    #[test]
    fn mixed_modal_commands_keep_unicode_offsets_valid() {
        let keys = [
            "h", "j", "k", "l", "0", "^", "$", "w", "W", "e", "b", "G", "g", "d", "y", "c", "i",
            "a", "s", "v", "V", "x", "p", "P", "u", "ctrl-r", ".", "f", "F", "t", "T", ";", ",",
            "(", ")", "{", "}", "%", "r", "~", "escape", "\"",
        ];
        let initial = "a\u{301}😀 word-one (two [three])\n\nnext ß 🦀\n";
        let mut editor = Editor::new(initial);
        let mut seed = 417_u64;
        for step in 0..4096 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            if editor.state.mode == Mode::Insert {
                editor.type_text("é");
                editor.keys(&["escape"]);
            }
            if step % 67 == 0 {
                editor.text = initial.into();
                editor.cursor = 0;
                editor.selection = 0..0;
                editor.state.observe(&editor.text, 0);
            }
            editor.keys(&[keys[(seed >> 32) as usize % keys.len()]]);
            assert!(editor.text.is_char_boundary(editor.cursor));
            assert!(editor.text.is_char_boundary(editor.selection.start));
            assert!(editor.text.is_char_boundary(editor.selection.end));
            assert!(editor.cursor <= editor.text.len());
            if editor.text.len() > 8192 {
                editor = Editor::new(initial);
            }
        }
    }
}

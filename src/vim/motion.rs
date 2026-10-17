use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

pub fn next(text: &str, at: usize) -> usize {
    at + text[at..].graphemes(true).next().map_or(0, str::len)
}

pub fn previous(text: &str, at: usize) -> usize {
    text[..at]
        .grapheme_indices(true)
        .next_back()
        .map_or(0, |(offset, _)| offset)
}

pub fn start(text: &str, at: usize) -> usize {
    text[..at].rfind('\n').map_or(0, |offset| offset + 1)
}

pub fn end(text: &str, at: usize) -> usize {
    let end = text[at..]
        .find('\n')
        .map_or(text.len(), |offset| at + offset);
    if end > 0 && text.as_bytes()[end - 1] == b'\r' {
        end - 1
    } else {
        end
    }
}

pub fn after_line(text: &str, at: usize) -> usize {
    text[at..]
        .find('\n')
        .map_or(text.len(), |offset| at + offset + 1)
}

pub fn first(text: &str, at: usize) -> usize {
    let begin = start(text, at);
    let line = &text[begin..end(text, begin)];
    begin + line.len() - line.trim_start_matches([' ', '\t']).len()
}

pub fn clip(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    if at == text.len() && text.ends_with('\n') {
        at = previous(text, at);
    }
    let line = start(text, at);
    let finish = end(text, line);
    // Native clicks can land inside a combining cluster; Normal mode cannot.
    text[line..finish]
        .grapheme_indices(true)
        .take_while(|(offset, _)| line + offset <= at)
        .last()
        .map_or(line, |(offset, _)| line + offset)
}

pub fn column(text: &str, at: usize) -> usize {
    text[start(text, at)..at].graphemes(true).count()
}

pub fn at_column(text: &str, line: usize, column: usize) -> usize {
    let finish = end(text, line);
    text[line..finish]
        .grapheme_indices(true)
        .nth(column)
        .map_or(finish, |(offset, _)| line + offset)
}

pub fn line_number(text: &str, row: usize) -> usize {
    if row == 0 {
        return 0;
    }
    let last = start(
        text,
        if text.ends_with('\n') {
            previous(text, text.len())
        } else {
            text.len()
        },
    );
    text.match_indices('\n')
        .nth(row - 1)
        .map_or(last, |(offset, _)| (offset + 1).min(last))
}

pub fn row(text: &str, at: usize) -> usize {
    text[..at].bytes().filter(|&byte| byte == b'\n').count()
}

pub fn line_range(text: &str, at: usize, count: usize) -> Range<usize> {
    let begin = start(text, at);
    let mut finish = begin;
    for _ in 0..count {
        let next = after_line(text, finish);
        if next == finish {
            break;
        }
        finish = next;
    }
    begin..finish
}

fn class(text: &str, at: usize, big: bool) -> u8 {
    match text[at..].chars().next() {
        None => 0,
        Some(ch) if ch.is_whitespace() => 0,
        Some(ch) if big || ch.is_alphanumeric() || ch == '_' => 1,
        _ => 2,
    }
}

pub fn word(text: &str, mut at: usize, direction: char, count: usize) -> usize {
    let big = direction.is_uppercase();
    for _ in 0..count {
        let before = at;
        match direction.to_ascii_lowercase() {
            'w' => {
                let kind = class(text, at, big);
                if kind != 0 {
                    while at < text.len() && class(text, at, big) == kind {
                        at = next(text, at);
                    }
                }
                while at < text.len() && class(text, at, big) == 0 {
                    // Empty lines are Vim word stops, unlike ordinary whitespace.
                    if at > before && start(text, at) == at && end(text, at) == at {
                        break;
                    }
                    at = next(text, at);
                }
            }
            'b' => {
                at = previous(text, at);
                while at > 0 && class(text, at, big) == 0 {
                    at = previous(text, at);
                }
                let kind = class(text, at, big);
                while at > 0 && class(text, previous(text, at), big) == kind {
                    at = previous(text, at);
                }
            }
            'e' => {
                at = next(text, at);
                while at < text.len() && class(text, at, big) == 0 {
                    at = next(text, at);
                }
                let kind = class(text, at, big);
                while at < text.len() {
                    let following = next(text, at);
                    if following == text.len() || class(text, following, big) != kind {
                        break;
                    }
                    at = following;
                }
            }
            _ => {}
        }
        if before == at {
            break;
        }
    }
    at
}

pub fn word_object(text: &str, at: usize, around: bool, big: bool, count: usize) -> Range<usize> {
    if at == text.len() {
        return at..at;
    }
    let kind = class(text, at, big);
    let mut begin = at;
    while begin > 0 && class(text, previous(text, begin), big) == kind {
        begin = previous(text, begin);
    }
    let mut finish = next(text, at);
    while finish < text.len() && class(text, finish, big) == kind {
        finish = next(text, finish);
    }
    for _ in 1..count {
        while finish < text.len() && class(text, finish, big) == 0 {
            finish = next(text, finish);
        }
        let kind = class(text, finish, big);
        while finish < text.len() && class(text, finish, big) == kind {
            finish = next(text, finish);
        }
    }
    if around {
        let original = finish;
        while finish < end(text, at) && class(text, finish, big) == 0 {
            finish = next(text, finish);
        }
        if original == finish {
            while begin > start(text, at) && class(text, previous(text, begin), big) == 0 {
                begin = previous(text, begin);
            }
        }
    }
    begin..finish
}

pub fn pair_object(
    text: &str,
    at: usize,
    object: char,
    around: bool,
    count: usize,
) -> Option<Range<usize>> {
    let (open, close) = match object {
        '(' | ')' | 'b' => ('(', ')'),
        '[' | ']' => ('[', ']'),
        '{' | '}' | 'B' => ('{', '}'),
        '<' | '>' => ('<', '>'),
        '"' | '\'' | '`' => (object, object),
        _ => return None,
    };
    let mut candidates = Vec::new();
    if open == close {
        let begin = start(text, at);
        let mut opening = None;
        let mut escaped = false;
        for (offset, ch) in text[begin..end(text, at)].char_indices() {
            if !escaped && ch == open {
                if let Some(left) = opening.take() {
                    candidates.push((left, begin + offset));
                } else {
                    opening = Some(begin + offset);
                }
            }
            escaped = ch == '\\' && !escaped;
        }
    } else {
        let mut stack = Vec::new();
        for (offset, ch) in text.char_indices() {
            if ch == open {
                stack.push(offset);
            } else if ch == close
                && let Some(left) = stack.pop()
            {
                candidates.push((left, offset));
            }
        }
    }
    candidates.retain(|&(left, right)| left <= at && at <= right);
    candidates.sort_by_key(|&(left, right)| right - left);
    let &(left, right) = candidates.get(count.saturating_sub(1))?;
    Some(if around {
        left..right + close.len_utf8()
    } else {
        left + open.len_utf8()..right
    })
}

pub fn matching_pair(text: &str, at: usize) -> Option<usize> {
    let (offset, ch) = text[at..end(text, at)]
        .char_indices()
        .find(|(_, ch)| "()[]{}".contains(*ch))?;
    let at = at + offset;
    let (open, close, forward) = match ch {
        '(' => ('(', ')', true),
        ')' => ('(', ')', false),
        '[' => ('[', ']', true),
        ']' => ('[', ']', false),
        '{' => ('{', '}', true),
        '}' => ('{', '}', false),
        _ => return None,
    };
    let mut depth = 0;
    let chars: Box<dyn Iterator<Item = (usize, char)> + '_> = if forward {
        Box::new(
            text[at..]
                .char_indices()
                .map(move |(offset, ch)| (at + offset, ch)),
        )
    } else {
        Box::new(text[..=at].char_indices().rev())
    };
    for (offset, ch) in chars {
        if ch == if forward { open } else { close } {
            depth += 1;
        }
        if ch == if forward { close } else { open } {
            depth -= 1;
            if depth == 0 {
                return Some(offset);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unicode_graphemes_words_and_nested_objects() {
        let text = "a\u{301}😀 word-one\r\n\r\nnext";
        assert_eq!(next(text, 0), 3);
        assert_eq!(previous(text, next(text, 3)), 3);
        assert_eq!(&text[word_object(text, 9, false, false, 1)], "word");
        assert_eq!(&text[word_object(text, 9, false, true, 1)], "word-one");
        assert_eq!(word(text, 0, 'w', 1), 3);
        assert_eq!(word(text, 3, 'w', 1), 8);
        assert_eq!(&text[end(text, 0)..after_line(text, 0)], "\r\n");
        let nested = "outer(one(two) three)";
        assert_eq!(
            &nested[pair_object(nested, 11, '(', false, 1).unwrap()],
            "two"
        );
        assert_eq!(
            &nested[pair_object(nested, 11, '(', false, 2).unwrap()],
            "one(two) three"
        );
        assert_eq!(matching_pair(nested, 5), Some(20));
    }
}

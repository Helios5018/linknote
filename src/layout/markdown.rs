use pulldown_cmark::{CowStr, Event, LinkType, Parser, Tag, TagEnd};
use std::{
    hash::{DefaultHasher, Hash, Hasher},
    ops::Range,
};

pub(super) fn events(text: &str) -> Vec<(Event<'_>, Range<usize>)> {
    parse(text).0
}

/// Parse the whole note once. `[[wiki links]]` are recognised in plain text runs
/// here instead of with pulldown-cmark's wikilink option, which can panic on some
/// unbalanced bracket input in 0.13.4. The second value hashes document-wide context that can
/// change how an unchanged block renders: reference link and footnote definitions.
/// Paired **label：** immediately followed by CJK text, which CommonMark's punctuation
/// flanking rule leaves literal, is accepted. Escapes, code and HTML are unchanged.
pub(super) fn parse(text: &str) -> (Vec<(Event<'_>, Range<usize>)>, u64) {
    let parser = Parser::new_ext(text, super::options());
    let mut context = DefaultHasher::new();
    let mut definitions: Vec<_> = parser
        .reference_definitions()
        .iter()
        .map(|(label, def)| {
            (
                label.to_owned(),
                def.dest.to_string(),
                def.title.as_ref().map(|t| t.to_string()),
            )
        })
        .collect();
    definitions.sort();
    definitions.hash(&mut context);
    let mut events = parser.into_offset_iter().peekable();
    let mut out = vec![];
    let mut code = false;
    while let Some((event, range)) = events.next() {
        if let Event::Start(Tag::FootnoteDefinition(label)) = &event {
            label.as_ref().hash(&mut context);
        }
        if matches!(event, Event::Start(Tag::CodeBlock(_))) {
            code = true;
        }
        if matches!(event, Event::End(TagEnd::CodeBlock)) {
            code = false;
        }
        if !code && matches!(&event, Event::Text(value) if value.as_ref() == &text[range.clone()]) {
            let mut end = range.end;
            while events.peek().is_some_and(|(e, r)| {
                r.start == end && matches!(e, Event::Text(s) if s.as_ref() == &text[r.clone()])
            }) {
                end = events.next().unwrap().1.end;
            }
            text_run(text, range.start..end, &mut out);
        } else {
            out.push((event, range));
        }
    }
    (out, context.finish())
}

fn text_run<'a>(text: &'a str, range: Range<usize>, out: &mut Vec<(Event<'a>, Range<usize>)>) {
    let mut pos = range.start;
    let mut scan = pos;
    while let Some(at) = text[scan..range.end].find("[[") {
        let open = scan + at;
        let inner_start = open + 2;
        let Some(close_at) = text[inner_start..range.end].find("]]") else {
            break;
        };
        let close = inner_start + close_at;
        let inner = &text[inner_start..close];
        if inner.trim().is_empty() || inner.contains(['[', ']', '\n']) {
            scan = inner_start;
            continue;
        }
        strong_run(text, pos..open, out);
        let (target, label_start) = match inner.find('|') {
            Some(bar) => (&inner[..bar], inner_start + bar + 1),
            None => (inner, inner_start),
        };
        let link = open..close + 2;
        out.push((
            Event::Start(Tag::Link {
                link_type: LinkType::WikiLink {
                    has_pothole: label_start != inner_start,
                },
                dest_url: CowStr::from(target),
                title: CowStr::from(""),
                id: CowStr::from(""),
            }),
            link.clone(),
        ));
        if label_start < close {
            out.push((
                Event::Text(CowStr::from(&text[label_start..close])),
                label_start..close,
            ));
        }
        out.push((Event::End(TagEnd::Link), link));
        pos = close + 2;
        scan = pos;
    }
    strong_run(text, pos..range.end, out);
}

fn strong_run<'a>(text: &'a str, range: Range<usize>, out: &mut Vec<(Event<'a>, Range<usize>)>) {
    if range.is_empty() {
        return;
    }
    let mut pos = range.start;
    let mut scan = pos;
    while let Some(at) = text[scan..range.end].find("**") {
        let open = scan + at;
        let Some(close_at) = text[open + 2..range.end].find("**") else {
            break;
        };
        let close = open + 2 + close_at;
        let inner = &text[open + 2..close];
        let triple = text[..open].ends_with('*')
            || text[open + 2..].starts_with('*')
            || text[..close].ends_with('*')
            || text[close + 2..].starts_with('*');
        if inner.is_empty()
            || inner.starts_with(char::is_whitespace)
            || inner.ends_with(char::is_whitespace)
            || triple
        {
            scan = open + 2;
            continue;
        }
        if pos < open {
            out.push((Event::Text(CowStr::from(&text[pos..open])), pos..open));
        }
        out.push((Event::Start(Tag::Strong), open..close + 2));
        out.push((Event::Text(CowStr::from(inner)), open + 2..close));
        out.push((Event::End(TagEnd::Strong), open..close + 2));
        pos = close + 2;
        scan = pos;
    }
    if pos < range.end {
        out.push((
            Event::Text(CowStr::from(&text[pos..range.end])),
            pos..range.end,
        ));
    }
}

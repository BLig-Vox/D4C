//! Autolink only prose. Markdown destinations, code, and image labels stay intact.
use linkify::{LinkFinder, LinkKind};
use pulldown_cmark::{Event, Parser, Tag, TagEnd};
use std::borrow::Cow;

pub fn markdown(text: &str) -> Cow<'_, str> {
    if !text.contains("http") {
        return Cow::Borrowed(text);
    }
    let mut finder = LinkFinder::new();
    finder.kinds(&[LinkKind::Url]);
    let mut result = String::new();
    let mut cursor = 0;
    let mut skip = 0;
    for (event, range) in Parser::new(text).into_offset_iter() {
        match event {
            Event::Start(Tag::Link { .. } | Tag::Image { .. } | Tag::CodeBlock(_)) => skip += 1,
            Event::End(TagEnd::Link | TagEnd::Image | TagEnd::CodeBlock) => skip -= 1,
            Event::Text(_) if skip == 0 => {
                for link in finder.links(&text[range.clone()]) {
                    if !(link.as_str().starts_with("https://")
                        || link.as_str().starts_with("http://"))
                    {
                        continue;
                    }
                    let start = range.start + link.start();
                    let end = range.start + link.end();
                    result.push_str(&text[cursor..start]);
                    result.push('<');
                    result.push_str(&text[start..end]);
                    result.push('>');
                    cursor = end;
                }
            }
            _ => {}
        }
    }
    if cursor == 0 {
        Cow::Borrowed(text)
    } else {
        result.push_str(&text[cursor..]);
        Cow::Owned(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn links_in_prose_preserve_existing_markup_and_punctuation() {
        let text = "Visit https://example.com/a. [Site](https://example.com) `https://code.test`\n\n```\nhttps://code.test\n```";
        assert_eq!(
            markdown(text),
            text.replacen("https://example.com/a", "<https://example.com/a>", 1)
        );
    }
}

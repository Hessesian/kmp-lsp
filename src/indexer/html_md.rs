//! HTML → Markdown conversion for the small subset of HTML that Javadoc embeds.
//!
//! Kept separate from `doc` (KDoc/Javadoc *tag* handling) so the two concerns
//! don't mix. Pure string transforms — no dependencies, no I/O.
//!
//! This is deliberately a tiny hand-rolled converter rather than a full HTML
//! parser: the input is the limited Javadoc subset (`<p> <br> <code> <pre>
//! <a> <ul>/<li> <b>/<i>` + headings + entities), and a real parser would
//! still treat a generic like `List<String>` as an unknown `<String>` element —
//! so the "is this actually a tag?" decision is a known-element allowlist either
//! way (see `is_known_html_tag`).

/// Convert the HTML that Javadoc embeds into Markdown. Kotlin KDoc rarely uses
/// HTML, so the fast path returns the input untouched when there's nothing to do.
pub(crate) fn html_to_markdown(text: &str) -> String {
    if !text.contains('<') && !text.contains('&') {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    let mut in_pre = false;
    while let Some(lt) = rest.find('<') {
        out.push_str(&rest[..lt]);
        rest = &rest[lt..];
        let Some(gt) = rest.find('>') else {
            out.push('<'); // stray '<', keep literally
            rest = &rest[1..];
            continue;
        };
        let tag = &rest[..=gt]; // the full "<…>" including brackets
        let raw = rest[1..gt].trim().to_ascii_lowercase();
        let after = &rest[gt + 1..];
        let closing = raw.starts_with('/');
        let name: String = raw
            .trim_start_matches('/')
            .chars()
            .take_while(char::is_ascii_alphanumeric)
            .collect();
        match name.as_str() {
            "pre" => {
                in_pre = !closing;
                out.push_str("\n```\n");
            }
            "code" | "tt" if !in_pre => out.push('`'),
            "b" | "strong" => out.push_str("**"),
            "i" | "em" => out.push('*'),
            "p" if !closing => out.push_str("\n\n"),
            "br" => out.push('\n'),
            "li" if !closing => out.push_str("\n- "),
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                out.push_str(if closing { "**\n" } else { "\n\n**" })
            }
            // Known structural HTML (ul/ol/a/div/span/table/…): strip, keep inner text.
            // Anything else is NOT a tag — e.g. a generic `List<String>` / `Map<K, V>`
            // or a stray `a < b` — so emit it literally instead of silently deleting it.
            _ => {
                if !is_known_html_tag(&name) {
                    out.push_str(tag);
                }
            }
        }
        rest = after;
    }
    out.push_str(rest);
    collapse_blank_lines(&decode_html_entities(&out))
}

/// Whether `name` (lowercased element name) is a recognized HTML element.
/// Used to decide whether an unrecognized-by-transform `<name>` should be
/// stripped (real tag) or kept literally (a generic / comparison that merely
/// looks like a tag).
fn is_known_html_tag(name: &str) -> bool {
    const KNOWN_HTML_TAGS: &[&str] = &[
        // Elements transformed above (listed for completeness).
        "p",
        "br",
        "code",
        "tt",
        "pre",
        "b",
        "strong",
        "i",
        "em",
        "li",
        "h1",
        "h2",
        "h3",
        "h4",
        "h5",
        "h6", // Structural / inline elements we strip, keeping inner text.
        "ul",
        "ol",
        "dl",
        "dt",
        "dd",
        "a",
        "div",
        "span",
        "table",
        "thead",
        "tbody",
        "tfoot",
        "tr",
        "td",
        "th",
        "caption",
        "col",
        "colgroup",
        "blockquote",
        "hr",
        "u",
        "s",
        "strike",
        "del",
        "ins",
        "sub",
        "sup",
        "small",
        "big",
        "kbd",
        "samp",
        "var",
        "cite",
        "q",
        "abbr",
        "address",
        "center",
        "font",
        "section",
        "article",
        "header",
        "footer",
        "nav",
        "aside",
        "main",
        "figure",
        "figcaption",
        "img",
        "wbr",
        "mark",
        "dfn",
        "time",
        "data",
        "ruby",
        "bdi",
        "bdo",
        "picture",
        "source",
        "summary",
        "details",
    ];
    KNOWN_HTML_TAGS.contains(&name)
}

/// Decode the handful of HTML entities Javadoc actually uses, plus numeric
/// (`&#39;`, `&#x2F;`) forms. Unknown entities are left as-is.
fn decode_html_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        // Entity names/refs are short; only treat a nearby ';' as a terminator.
        if let Some(semi) = rest.find(';').filter(|&i| i <= 10) {
            let entity = &rest[1..semi];
            let decoded = match entity {
                "lt" => Some('<'),
                "gt" => Some('>'),
                "amp" => Some('&'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some(' '),
                _ => entity.strip_prefix('#').and_then(|num| {
                    let code = match num.strip_prefix(['x', 'X']) {
                        Some(hex) => u32::from_str_radix(hex, 16).ok(),
                        None => num.parse::<u32>().ok(),
                    };
                    code.and_then(char::from_u32)
                }),
            };
            if let Some(ch) = decoded {
                out.push(ch);
                rest = &rest[semi + 1..];
                continue;
            }
        }
        out.push('&'); // not a recognised entity
        rest = &rest[1..];
    }
    out.push_str(rest);
    out
}

/// Collapse runs of 3+ newlines down to a blank-line separator (2 newlines).
fn collapse_blank_lines(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut newlines = 0usize;
    for ch in s.chars() {
        if ch == '\n' {
            newlines += 1;
            if newlines <= 2 {
                out.push('\n');
            }
        } else {
            newlines = 0;
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
#[path = "html_md_tests.rs"]
mod tests;

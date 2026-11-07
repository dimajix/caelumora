//! What a page says, as text (issue 544): an HTML page as Markdown, without what only a browser
//! needs or what stands around the content on every page of a site - scripts, styles, navigation,
//! footer, sidebars -; plain text and JSON as they are.

use htmd::HtmlToMarkdown;
use reqwest::Url;

/// The elements of an HTML page left out of its text: what only a browser needs, the controls of
/// forms, and what stands around the content - navigation, a site's footer, a sidebar. Not a form
/// itself, which on some sites holds the whole page, nor a header, which holds an article's title
/// too.
const LEFT_OUT: [&str; 14] = [
    "head", "script", "style", "noscript", "template", "svg", "canvas", "iframe", "button",
    "select", "textarea", "nav", "footer", "aside",
];

/// A page as its model reads it.
#[derive(Debug)]
pub(super) struct Page {
    /// Where it was found, after redirects.
    pub(super) url: Url,
    /// Its title, if it names one.
    pub(super) title: Option<String>,
    pub(super) text: String,
}

/// Whether a page of `media_type` - lower case, without its parameters - can be read as text.
pub(super) fn readable(media_type: &str) -> bool {
    is_html(media_type)
        || media_type.starts_with("text/")
        || media_type == "application/json"
        || media_type.ends_with("+json")
}

fn is_html(media_type: &str) -> bool {
    matches!(media_type, "text/html" | "application/xhtml+xml")
}

/// The page found at `url`, of `media_type`, whose body is `body`: as Markdown if it is HTML, as
/// it is otherwise.
pub(super) fn page(url: Url, media_type: &str, body: &str) -> Page {
    if !is_html(media_type) {
        return Page { url, title: None, text: body.to_string() };
    }
    let converter = HtmlToMarkdown::builder().skip_tags(LEFT_OUT.to_vec()).build();
    let title = title_of(body, &converter);
    // A page html5ever cannot read is rare, as it reads what a browser reads; it is given as
    // it came rather than not at all.
    let text = converter.convert(body).unwrap_or_else(|_| body.to_string());
    Page { url, title, text }
}

/// The title `html` names, its entities resolved by `converter`; none for one that is empty.
fn title_of(html: &str, converter: &HtmlToMarkdown) -> Option<String> {
    // Lower case in ASCII alone, so that every position is one of `html` too.
    let lower = html.to_ascii_lowercase();
    let tag = lower.find("<title")?;
    let start = tag + lower[tag..].find('>')? + 1;
    let end = start + lower[start..].find("</title")?;
    let title = converter.convert(&html[start..end]).ok()?;
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    (!title.is_empty()).then_some(title)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url() -> Url {
        Url::parse("https://example.org/page").unwrap()
    }

    #[test]
    fn a_page_of_html_is_read_as_markdown_without_what_stands_around_its_content() {
        let html = "<html><head><title>The &amp; Title</title><style>p { }</style></head><body>\
                    <nav><a href=\"/\">Home</a></nav><form><button>Send</button>\
                    <header><h1>Heading</h1></header><p>Some <b>bold</b> text.</p><script>alert(1)</script>\
                    </form><footer>Imprint</footer></body></html>";
        let page = page(url(), "text/html", html);
        assert_eq!(page.title.as_deref(), Some("The & Title"));
        assert!(page.text.contains("# Heading"), "{}", page.text);
        assert!(page.text.contains("Some **bold** text."), "{}", page.text);
        for left_out in ["Home", "Send", "alert", "Imprint", "p { }", "Title"] {
            assert!(!page.text.contains(left_out), "{left_out} in {}", page.text);
        }
    }

    #[test]
    fn text_and_json_are_read_as_they_are() {
        let page = page(url(), "application/json", "{\"a\": \"<b>\"}");
        assert_eq!(page.text, "{\"a\": \"<b>\"}");
        assert_eq!(page.title, None);
        for readable in ["text/plain", "text/markdown", "application/json", "application/ld+json"] {
            assert!(super::readable(readable), "{readable}");
        }
        for unreadable in ["application/pdf", "image/png", "application/octet-stream"] {
            assert!(!super::readable(unreadable), "{unreadable}");
        }
    }

    #[test]
    fn a_page_without_a_title_has_none() {
        assert_eq!(page(url(), "text/html", "<p>text</p>").title, None);
        assert_eq!(page(url(), "text/html", "<title> </title><p>text</p>").title, None);
    }
}

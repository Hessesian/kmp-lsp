//! Unit tests for the doc-comment extraction functions in `doc.rs`.
use super::extract_doc_comment;

fn lines(src: &str) -> Vec<String> {
    src.lines().map(String::from).collect()
}

#[test]
fn extract_doc_comment_out_of_bounds_safety() {
    let empty_lines: Vec<String> = Vec::new();
    // Test that passing a line number beyond the empty bounds returns None safely without panicking
    assert_eq!(extract_doc_comment(&empty_lines, 165), None);
    assert_eq!(extract_doc_comment(&empty_lines, 0), None);
}

#[test]
fn kdoc_simple_block_comment() {
    let src = r#"
/**
 * Does something useful.
 */
fun doThing() {}"#;
    let ls = lines(src);
    let decl = ls.iter().position(|l| l.contains("fun doThing")).unwrap();
    let doc = extract_doc_comment(&ls, decl).unwrap();
    assert!(doc.contains("Does something useful"), "got: {doc}");
    // extract_doc_comment returns plain text; no code block here
    assert!(!doc.contains("```"), "got: {doc}");
}

#[test]
fn kdoc_with_params_and_return() {
    let src = r#"
/**
 * Fetches the widget.
 *
 * @param id The widget identifier.
 * @param flag Whether to refresh.
 * @return The widget or null.
 */
fun getWidget(id: Int, flag: Boolean): Widget? = null"#;
    let ls = lines(src);
    let decl = ls.iter().position(|l| l.contains("fun getWidget")).unwrap();
    let doc = extract_doc_comment(&ls, decl).unwrap();
    assert!(doc.contains("Fetches the widget"), "got: {doc}");
    assert!(doc.contains("**Parameters**"), "got: {doc}");
    assert!(doc.contains("`id`"), "got: {doc}");
    assert!(doc.contains("`flag`"), "got: {doc}");
    assert!(doc.contains("**Returns**"), "got: {doc}");
}

#[test]
fn kdoc_skips_annotations() {
    let src = r#"
/**
 * Annotated function.
 */
@Suppress("unused")
@JvmStatic
fun annotated() {}"#;
    let ls = lines(src);
    let decl = ls.iter().position(|l| l.contains("fun annotated")).unwrap();
    let doc = extract_doc_comment(&ls, decl).unwrap();
    assert!(doc.contains("Annotated function"), "got: {doc}");
}

#[test]
fn kdoc_no_comment_returns_none() {
    let src = "fun plain() {}";
    let ls = lines(src);
    assert!(extract_doc_comment(&ls, 0).is_none());
}

#[test]
fn kdoc_line_comments() {
    let src = r#"// Short description.
// More detail.
fun withLineDoc() {}"#;
    let ls = lines(src);
    let decl = 2;
    let doc = extract_doc_comment(&ls, decl).unwrap();
    assert!(doc.contains("Short description"), "got: {doc}");
    assert!(doc.contains("More detail"), "got: {doc}");
}

#[test]
fn kdoc_preserves_utf_8_text() {
    let src = r#"
/**
 * Lorem Ipsum является стандартной "рыбой" для текстов на латинице с начала XVI века.
 * См. также [Widget] для деталей.
 */
fun doThing() {}"#;
    let ls = lines(src);
    let decl = ls.iter().position(|l| l.contains("fun doThing")).unwrap();
    let doc = extract_doc_comment(&ls, decl).unwrap();
    assert!(
        doc.contains(
            r#"Lorem Ipsum является стандартной "рыбой" для текстов на латинице с начала XVI века."#
        ),
        "got: {doc}"
    );
    assert!(doc.contains("`Widget`"), "got: {doc}");
    assert!(!doc.contains('\u{fffd}'), "got: {doc}");
}

#[test]
fn kdoc_inline_code_and_links() {
    let src = r#"
/**
 * Use {@code Foo.bar()} or [Baz] to achieve this.
 */
fun example() {}"#;
    let ls = lines(src);
    let decl = ls.iter().position(|l| l.contains("fun example")).unwrap();
    let doc = extract_doc_comment(&ls, decl).unwrap();
    assert!(doc.contains("`Foo.bar()`"), "got: {doc}");
    assert!(doc.contains("`Baz`"), "got: {doc}");
}

// ── HTML Javadoc → Markdown ───────────────────────────────────────────────────

#[test]
fn javadoc_html_paragraph_code_converted() {
    // Mirrors javax.inject @Inject style docs (HTML Javadoc).
    let doc = "Identifies injectable constructors.<p>Annotate with <code>@Inject</code>.";
    let md = super::format_doc_comment(doc);
    assert!(!md.contains("<p>"), "raw <p> leaked: {md:?}");
    assert!(!md.contains("<code>"), "raw <code> leaked: {md:?}");
    assert!(md.contains("`@Inject`"), "code span not backticked: {md:?}");
}

#[test]
fn javadoc_html_entities_decoded() {
    let md = super::format_doc_comment("Returns a List&lt;String&gt; &amp; never null.");
    assert!(md.contains("List<String>"), "lt/gt not decoded: {md:?}");
    assert!(md.contains("& never"), "amp not decoded: {md:?}");
    assert!(!md.contains("&lt;"), "entity leaked: {md:?}");
}

#[test]
fn javadoc_html_anchor_list_converted() {
    let md = super::format_doc_comment(
        "See <a href=\"http://x\">the docs</a>.<ul><li>one</li><li>two</li></ul>",
    );
    assert!(!md.contains("<a "), "anchor tag leaked: {md:?}");
    assert!(md.contains("the docs"), "anchor text dropped: {md:?}");
    assert!(
        md.contains("- one") && md.contains("- two"),
        "list not converted: {md:?}"
    );
}

#[test]
fn jar_doc_blank_renders_empty() {
    assert_eq!(super::format_doc_comment("   \n  "), "");
}

#[test]
fn kdoc_without_html_is_unchanged_text() {
    // Fast path: plain KDoc prose with no tags/HTML survives intact.
    let md = super::format_doc_comment("Just a plain sentence.");
    assert_eq!(md, "Just a plain sentence.");
}

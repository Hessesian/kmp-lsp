use super::html_to_markdown;

#[test]
fn plain_text_unchanged() {
    assert_eq!(
        html_to_markdown("Just a plain sentence."),
        "Just a plain sentence."
    );
}

#[test]
fn paragraph_code_converted() {
    let md = html_to_markdown(
        "Identifies injectable constructors.<p>Annotate with <code>@Inject</code>.",
    );
    assert!(!md.contains("<p>"), "raw <p> leaked: {md:?}");
    assert!(!md.contains("<code>"), "raw <code> leaked: {md:?}");
    assert!(md.contains("`@Inject`"), "code span not backticked: {md:?}");
}

#[test]
fn anchor_list_converted() {
    let md =
        html_to_markdown("See <a href=\"http://x\">the docs</a>.<ul><li>one</li><li>two</li></ul>");
    assert!(!md.contains("<a "), "anchor tag leaked: {md:?}");
    assert!(md.contains("the docs"), "anchor text dropped: {md:?}");
    assert!(
        md.contains("- one") && md.contains("- two"),
        "list not converted: {md:?}"
    );
}

#[test]
fn entities_decoded() {
    let md = html_to_markdown("Returns a List&lt;String&gt; &amp; never null.");
    assert!(md.contains("List<String>"), "lt/gt not decoded: {md:?}");
    assert!(md.contains("& never"), "amp not decoded: {md:?}");
}

// ── Regression: generic angle brackets must NOT be treated as HTML tags ────────

#[test]
fn generic_type_not_stripped() {
    // The bug this module fixes: `<String>` looks like a tag but isn't HTML.
    let md = html_to_markdown("Returns a List<String> of items.");
    assert!(
        md.contains("List<String>"),
        "generic stripped as a tag: {md:?}"
    );
}

#[test]
fn nested_generic_not_stripped() {
    let md = html_to_markdown("A Map<K, V> and a Flow<List<Item>>.");
    assert!(md.contains("Map<K, V>"), "Map generic stripped: {md:?}");
    assert!(md.contains("List<Item>"), "nested generic stripped: {md:?}");
}

#[test]
fn generic_mixed_with_real_html() {
    let md = html_to_markdown("<p>Builds a Provider<T> instance.</p>");
    assert!(
        md.contains("Provider<T>"),
        "generic stripped alongside <p>: {md:?}"
    );
    assert!(!md.contains("<p>"), "real <p> not converted: {md:?}");
}

#[test]
fn stray_lt_kept() {
    let md = html_to_markdown("Use when a < b holds.");
    assert!(md.contains("a < b"), "stray '<' lost: {md:?}");
}

#[test]
fn unknown_tag_kept_literal() {
    // An unrecognized element name that isn't real HTML is kept as-is.
    assert!(html_to_markdown("see <Foo> here").contains("<Foo>"));
}

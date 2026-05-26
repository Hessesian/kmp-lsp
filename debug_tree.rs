use tree_sitter_kotlin;

fn main() {
    let code = r#"
class User(val name: String) {
    fun rename() {
        name = "Alice"
    }
}
"#;

    let code2 = r#"
fun test() {
    listOf(1, 2).forEach { item ->
        item = 3
    }
}
"#;

    let code3 = r#"
class User(var name: String)
fun test() {
    val user = User("Bob")
    user.name = "Alice"
}
"#;

    let code4 = r#"
fun test() {
    listOf(1, 2).forEach {
        println(it)
        it = 3
    }
}
"#;

    println!("=== Code 1: class val param ===");
    print_tree(code);

    println!("\n=== Code 2: lambda param ===");
    print_tree(code2);

    println!("\n=== Code 3: navigation assignment ===");
    print_tree(code3);

    println!("\n=== Code 4: implicit 'it' ===");
    print_tree(code4);
}

fn print_tree(code: &str) {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(tree_sitter_kotlin::language())
        .unwrap();
    let tree = parser.parse(code, None).unwrap();
    print_node(tree.root_node(), code.as_bytes(), 0);
}

fn print_node(node: tree_sitter::Node, bytes: &[u8], indent: usize) {
    let prefix = "  ".repeat(indent);
    let text = node.utf8_text(bytes).unwrap_or("");
    let short_text = if text.len() > 40 {
        format!("{}...", &text[0..40])
    } else {
        text.to_string()
    }
    .replace('\n', "\\n");

    println!(
        "{}{} [{}] named={} extra={}",
        prefix,
        node.kind(),
        short_text,
        node.is_named(),
        node.is_extra()
    );

    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            print_node(cursor.node(), bytes, indent + 1);
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
}

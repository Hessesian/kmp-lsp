use super::descendants;
use tree_sitter::Node;

fn parse(source: &str) -> tree_sitter::Tree {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_kotlin::language())
        .expect("kotlin grammar loads");
    parser.parse(source, None).expect("source parses")
}

/// The recursion this module replaces, kept as the oracle it is checked against.
fn recursive_preorder<'tree>(node: Node<'tree>, out: &mut Vec<Node<'tree>>) {
    out.push(node);
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        recursive_preorder(child, out);
    }
}

#[test]
fn visits_exactly_what_the_recursive_walk_visits_in_the_same_order() {
    let tree = parse(
        "package app\n\
         import a.b.C\n\
         class Holder(val x: Int) {\n\
             fun f(a: Int): Int {\n\
                 val b = a + 1\n\
                 if (b > 0) { println(b) } else { println(-b) }\n\
                 return b\n\
             }\n\
         }\n",
    );
    let mut expected = Vec::new();
    recursive_preorder(tree.root_node(), &mut expected);

    let actual: Vec<Node> = descendants(tree.root_node()).collect();

    assert_eq!(
        actual.len(),
        expected.len(),
        "visited {} nodes, recursive walk visited {}",
        actual.len(),
        expected.len()
    );
    for (position, (actual, expected)) in actual.iter().zip(expected.iter()).enumerate() {
        assert_eq!(
            actual.id(),
            expected.id(),
            "diverged at position {position}: got `{}`, expected `{}`",
            actual.kind(),
            expected.kind()
        );
    }
}

#[test]
fn a_childless_root_yields_only_itself() {
    let tree = parse("");
    let root = tree.root_node();
    let visited: Vec<Node> = descendants(root).collect();
    assert_eq!(visited.len(), 1, "got {visited:?}");
    assert_eq!(visited[0].id(), root.id());
}

#[test]
fn the_walk_ends_rather_than_escaping_into_a_subtree_it_started_below_the_root() {
    // Starting at an inner node must not walk out into that node's siblings.
    let tree = parse("fun first() { println(1) }\nfun second() { println(2) }\n");
    let root = tree.root_node();
    let first_function = root.child(0).expect("a first function");

    let visited: Vec<Node> = descendants(first_function).collect();

    assert!(
        visited.iter().all(|node| node.id() == first_function.id()
            || first_function.byte_range().contains(&node.start_byte())),
        "the walk escaped the subtree it was given"
    );
    let mut expected = Vec::new();
    recursive_preorder(first_function, &mut expected);
    assert_eq!(visited.len(), expected.len());
}

/// The reason this module exists: depth must cost no stack, so no walk built on
/// it needs a depth cap.
#[test]
fn a_pathologically_deep_tree_costs_no_stack() {
    let n = 60_000; // ~100x the depth cap the recursive walkers needed
    let mut source = String::from("fun f() {\n    val x = 1");
    for _ in 0..n {
        source.push_str("+1");
    }
    source.push_str("\n}\n");

    let handle = std::thread::Builder::new()
        // A 16th of the default stack: an iterative walk should not care, and a
        // recursive one dies here at a few hundred levels, let alone 60,000.
        .stack_size(512 * 1024)
        .spawn(move || {
            let tree = parse(&source);
            descendants(tree.root_node()).count()
        })
        .expect("thread spawns");
    // A stack overflow aborts the process rather than failing this join.
    let visited = handle.join().expect("must not overflow the stack");
    assert!(
        visited > n,
        "expected to visit every node of a {n}-deep tree, saw {visited}"
    );
}

#[test]
fn a_leaf_root_with_siblings_yields_only_itself() {
    // A leaf has no child to descend into, so the walk immediately looks for a
    // sibling — which for a leaf `root` lies outside the subtree it was given.
    let tree = parse("fun first() {}\nfun second() {}\n");
    let first_function = tree.root_node().child(0).expect("a first function");
    let leaf = first_function.child(0).expect("the `fun` keyword");
    assert_eq!(leaf.child_count(), 0, "test needs a genuine leaf");
    assert!(
        leaf.next_sibling().is_some(),
        "test needs the leaf to have a sibling"
    );

    let visited: Vec<Node> = descendants(leaf).collect();

    assert_eq!(
        visited.len(),
        1,
        "a leaf is its own whole subtree, got {:?}",
        visited.iter().map(|node| node.kind()).collect::<Vec<_>>()
    );
    assert_eq!(visited[0].id(), leaf.id());
}

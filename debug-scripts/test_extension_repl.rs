:dep kmp-lsp = { path = "." }
use kmp_lsp::indexer::Indexer;
use tower_lsp::lsp_types::Url;

let idx = Indexer::new();
let uri = Url::parse("file:///test.kt").unwrap();
let src = "class IMockProvider\nfun IMockProvider.loadJSONFromAssets(path: String): Any = TODO()\nclass Foo(ctx: IMockProvider) { val r = ctx.loadJSONFromAssets() }";
idx.index_content(&uri, src);
idx.store_live_tree(&uri, src);

if let Some(entries) = idx.extension_by_receiver.get("IMockProvider") {
    println!("extensions on IMockProvider: {}", entries.len());
    for e in entries.iter() { println!("  {} ({})", e.name, e.file_uri); }
}

let keys: Vec<_> = idx.definitions.iter().map(|e| e.key().clone()).collect();
println!("def keys: {:?}", keys);

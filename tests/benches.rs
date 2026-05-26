//! Micro-benchmarks for kotlin-lsp CLI operations.
//! Run: cargo test --test benches -- --ignored --nocapture

use std::process::Command;
use std::time::Instant;

const BIN: &str = env!("CARGO_BIN_EXE_kotlin-lsp");
const ITERATIONS: u32 = 5;

#[test]
#[ignore]
fn bench_check_small_file() {
    let mut total = 0u128;
    for _ in 0..ITERATIONS {
        let start = Instant::now();
        let _ = Command::new(BIN).args(["check", "src/main.rs"]).output();
        total += start.elapsed().as_micros();
    }
    println!("check: {} us avg", total / ITERATIONS as u128);
}

#[test]
#[ignore]
fn bench_inject_small_file() {
    let mut total = 0u128;
    for _ in 0..ITERATIONS {
        let start = Instant::now();
        let _ = Command::new(BIN).args(["inject", "src/main.rs"]).output();
        total += start.elapsed().as_micros();
    }
    println!("inject: {} us avg", total / ITERATIONS as u128);
}

#[test]
#[ignore]
fn bench_find_small() {
    let mut total = 0u128;
    for _ in 0..ITERATIONS {
        let start = Instant::now();
        let _ = Command::new(BIN).args(["find", "main"]).output();
        total += start.elapsed().as_micros();
    }
    println!("find: {} us avg", total / ITERATIONS as u128);
}

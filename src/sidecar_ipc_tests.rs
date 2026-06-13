#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Write};
    use std::path::PathBuf;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    fn gradle_cache_jars() -> Vec<PathBuf> {
        let Ok(home) = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")) else {
            return Vec::new();
        };
        let cache = PathBuf::from(&home).join(".gradle/caches/modules-2/files-2.1");
        if !cache.exists() {
            return Vec::new();
        }
        let output = Command::new("find")
            .args([
                cache.to_str().unwrap(),
                "-name",
                "*.jar",
                "!",
                "-name",
                "*-sources*.jar",
                "!",
                "-name",
                "*-javadoc*.jar",
                "-type",
                "f",
            ])
            .output()
            .expect("find command failed");
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|l| !l.is_empty())
            .map(PathBuf::from)
            .collect()
    }

    fn launch_sidecar() -> Option<(
        std::io::BufWriter<std::process::ChildStdin>,
        BufReader<std::process::ChildStdout>,
        std::process::Child,
    )> {
        let path = PathBuf::from(std::env::var("HOME").unwrap_or_default())
            .join(".cargo/bin/kmp-jar-indexer");
        if !path.exists() {
            eprintln!("[test] sidecar not found at {:?}", path);
            return None;
        }
        eprintln!("[test] launching sidecar: {:?}", path);
        let mut child = Command::new(&path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .ok()?;
        let stdin = std::io::BufWriter::new(child.stdin.take()?);
        let stdout = BufReader::new(child.stdout.take()?);
        Some((stdin, stdout, child))
    }

    fn drain_sidecar(
        mut child: std::process::Child,
        mut stdin: std::io::BufWriter<std::process::ChildStdin>,
        mut stdout: BufReader<std::process::ChildStdout>,
    ) {
        let _ = stdin.write_all(b"{\"shutdown\":true}\n");
        let _ = stdin.flush();
        let reader = std::thread::spawn(move || {
            let mut buf = String::new();
            while let Ok(n) = stdout.read_line(&mut buf) {
                if n == 0 {
                    break;
                }
                buf.clear();
            }
        });
        let start = Instant::now();
        loop {
            if start.elapsed() > Duration::from_secs(10) {
                eprintln!("[test] sidecar stuck, killing");
                let _ = child.kill();
                break;
            }
            if let Ok(Some(status)) = child.try_wait() {
                eprintln!("[test] sidecar exited: {}", status);
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        let _ = reader.join();
    }

    /// Send one JAR, read one response, repeat. This identifies which specific JAR hangs.
    #[test]
    fn test_sidecar_sequential_all_jars_ipc() {
        let Some((stdin, stdout, child)) = launch_sidecar() else {
            eprintln!("[test] sidecar not found — skipping");
            return;
        };
        let jars = gradle_cache_jars();
        if jars.len() < 10 {
            eprintln!("[test] fewer than 10 JARs found");
            drain_sidecar(child, stdin, stdout);
            return;
        }
        let total = jars.len();
        eprintln!("[test] sequential indexing of ALL {} JARs", total);

        let mut stdin = stdin;
        let mut stdout = stdout;
        let start = Instant::now();
        let timeout_per_jar = Duration::from_secs(30);
        let mut responses = 0;
        let mut last_ok_jar = String::new();

        for (i, jar) in jars.iter().enumerate() {
            let jar_name = jar
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let jar_path = jar.to_str().unwrap();
            eprintln!("[test] [{}/{}] {}", i + 1, total, jar_name);

            let req = format!("{{\"jar\":\"{}\"}}\n", jar_path);
            stdin.write_all(req.as_bytes()).unwrap();
            stdin.flush().unwrap();

            let jar_start = Instant::now();
            loop {
                if jar_start.elapsed() > timeout_per_jar {
                    eprintln!("[test] TIMEOUT on jar [{}/{}]: {}", i + 1, total, jar_name);
                    eprintln!("[test] Full path: {}", jar_path);
                    eprintln!(
                        "[test] Last successful jar #{} was: {}",
                        responses, last_ok_jar
                    );
                    panic!(
                        "sidecar hung on jar {} of {} ({})\nlast OK was: {}",
                        i + 1,
                        total,
                        jar_name,
                        last_ok_jar
                    );
                }
                let mut line = String::new();
                match stdout.read_line(&mut line) {
                    Ok(0) => {
                        eprintln!(
                            "[test] SIDECAR CLOSED on jar [{}/{}]: {}",
                            i + 1,
                            total,
                            jar_name
                        );
                        eprintln!("[test] Last successful was: {}", last_ok_jar);
                        panic!(
                            "sidecar closed stdout on jar {} of {} ({})",
                            i + 1,
                            total,
                            jar_name
                        );
                    }
                    Ok(_) if line.trim().is_empty() => continue,
                    Ok(_) => {
                        let parsed: Result<Vec<serde_json::Value>, _> = serde_json::from_str(&line);
                        match parsed {
                            Ok(_) => {
                                responses += 1;
                                last_ok_jar = jar_name.clone();
                            }
                            Err(e) => {
                                eprintln!(
                                    "[test] PARSE ERROR: {} — {}",
                                    e,
                                    &line[..line.len().min(100)]
                                );
                            }
                        }
                        break;
                    }
                    Err(e) => panic!("read error on {}: {}", jar_name, e),
                }
            }
        }

        let elapsed = start.elapsed();
        eprintln!("[test] PASSED — {} JARs in {:?}", responses, elapsed);
        drain_sidecar(child, stdin, stdout);
    }
}

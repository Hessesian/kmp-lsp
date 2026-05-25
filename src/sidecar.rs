//! Long-running `kotlin-jar-indexer` sidecar process.
//!
//! Spawned once per `Indexer` instance; kept alive as a daemon.
//! Communicates via newline-delimited JSON on stdin/stdout:
//!
//! ```text
//! → {"jar":"/path/to/foo.jar"}\n
//! ← [{"name":"Column","kind":"fun","container":"ColumnKt","detail":"fun Column(...)"},...]\n
//! → {"shutdown":true}\n
//! ```
//!
//! On crash the handle is set to `None` — callers get no symbols for that run.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(crate) struct SidecarSymbol {
    pub name: String,
    pub kind: String,
    pub container: String,
    pub detail: String,
    #[serde(default)]
    pub doc: String,
}

pub(crate) struct SidecarHandle {
    child: Child,
    stdin: std::io::BufWriter<ChildStdin>,
    stdout: BufReader<ChildStdout>,
}

impl SidecarHandle {
    /// Probe for a usable sidecar binary and start it.
    ///
    /// Probe order:
    /// 1. `kotlin-jar-indexer` native binary adjacent to the running executable
    /// 2. `java -jar kotlin-jar-indexer.jar` adjacent to the running executable
    ///
    /// Returns `None` when neither is found or the process fails to start.
    pub(crate) fn try_launch() -> Option<Self> {
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_owned()))?;

        if let Some(handle) = Self::launch_native(&exe_dir.join("kotlin-jar-indexer")) {
            log::info!("sidecar: launched native kotlin-jar-indexer");
            return Some(handle);
        }
        let jar_path = exe_dir.join("kotlin-jar-indexer.jar");
        if let Some(handle) = Self::launch_jar(&jar_path) {
            log::info!("sidecar: launched kotlin-jar-indexer.jar via java");
            return Some(handle);
        }

        // Fallback: check ~/.cargo/bin/ so debug builds (target/debug/) find an
        // already-installed sidecar without requiring a full `cargo install`.
        if let Some(cargo_bin) = crate::util::home_dir().map(|h| h.join(".cargo").join("bin")) {
            if let Some(handle) = Self::launch_native(&cargo_bin.join("kotlin-jar-indexer")) {
                log::info!("sidecar: launched native kotlin-jar-indexer from ~/.cargo/bin");
                return Some(handle);
            }
            let fallback_jar = cargo_bin.join("kotlin-jar-indexer.jar");
            if let Some(handle) = Self::launch_jar(&fallback_jar) {
                log::info!("sidecar: launched kotlin-jar-indexer.jar from ~/.cargo/bin");
                return Some(handle);
            }
        }

        log::debug!("sidecar: no kotlin-jar-indexer found — JAR symbol quality degraded");
        None
    }

    fn launch_native(path: &Path) -> Option<Self> {
        if !path.exists() {
            return None;
        }
        Self::spawn(&mut Command::new(path))
    }

    fn launch_jar(path: &Path) -> Option<Self> {
        if !path.exists() {
            return None;
        }
        let java = find_java()?;
        Self::spawn(Command::new(java).args(["-jar", path.to_str()?]))
    }

    fn spawn(cmd: &mut Command) -> Option<Self> {
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .ok()?;
        let stdin = std::io::BufWriter::new(child.stdin.take()?);
        let stdout = BufReader::new(child.stdout.take()?);
        Some(Self {
            child,
            stdin,
            stdout,
        })
    }

    /// Send one JAR path to the sidecar and receive the symbol list.
    /// Returns `Err` on any I/O or parse failure; caller should set the
    /// handle to `None` and stop using it.
    pub(crate) fn index_jar(&mut self, path: &Path) -> Result<Vec<SidecarSymbol>, String> {
        #[derive(serde::Serialize)]
        struct JarRequest<'a> {
            jar: &'a str,
        }

        let path_str = path
            .to_str()
            .ok_or_else(|| format!("non-UTF-8 path: {:?}", path))?;

        let mut req =
            serde_json::to_string(&JarRequest { jar: path_str }).map_err(|e| e.to_string())?;
        req.push('\n');

        self.stdin
            .write_all(req.as_bytes())
            .map_err(|e| e.to_string())?;
        self.stdin.flush().map_err(|e| e.to_string())?;

        let mut line = String::new();
        self.stdout
            .read_line(&mut line)
            .map_err(|e| e.to_string())?;

        if line.is_empty() {
            return Err("sidecar closed stdout unexpectedly".to_owned());
        }
        serde_json::from_str::<Vec<SidecarSymbol>>(&line).map_err(|e| e.to_string())
    }
}

impl Drop for SidecarHandle {
    fn drop(&mut self) {
        let _ = self.stdin.write_all(b"{\"shutdown\":true}\n");
        let _ = self.stdin.flush();
        let _ = self.child.wait();
    }
}

/// Locate a `java` executable: check `$JAVA_HOME/bin/java` first, then PATH.
fn find_java() -> Option<PathBuf> {
    if let Ok(home) = std::env::var("JAVA_HOME") {
        let candidate = PathBuf::from(home).join("bin").join("java");
        if candidate.exists() {
            return Some(candidate);
        }
    }
    // Verify `java` is on PATH by running it with a no-op argument.
    if Command::new("java")
        .arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
    {
        return Some(PathBuf::from("java"));
    }
    None
}

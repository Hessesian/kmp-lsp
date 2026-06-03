//! CLI `new-file` subcommand — generate files from templates.
//!
//! Supported templates: `class`, `interface`, `object`, `data-class`, `sealed-class`,
//! `enum-class`, `activity`, `composable`, `viewmodel`.

use std::path::{Path, PathBuf};

/// Generate a file from the given template and write it to disk.
pub(crate) fn run_new_file(
    template: &str,
    name: &str,
    package: Option<&str>,
    directory: Option<&Path>,
) {
    let content = match template {
        "class" => generate_class(name, package),
        "data-class" => generate_data_class(name, package),
        "interface" => generate_interface(name, package),
        "object" => generate_object(name, package),
        "sealed-class" => generate_sealed_class(name, package),
        "enum-class" => generate_enum_class(name, package),
        "activity" => generate_activity(name, package),
        "composable" => generate_composable(name, package),
        "viewmodel" => generate_viewmodel(name, package),
        _ => {
            eprintln!("Unknown template: {template}");
            eprintln!("Supported: class, data-class, interface, object, sealed-class, enum-class, activity, composable, viewmodel");
            return;
        }
    };

    let dir = directory
        .map(|d| d.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let file_path = dir.join(format!("{name}.kt"));

    if let Some(p) = file_path.parent() {
        if let Err(e) = std::fs::create_dir_all(p) {
            eprintln!("Failed to create directory {}: {e}", p.display());
            return;
        }
    }

    match std::fs::write(&file_path, &content) {
        Ok(_) => println!("Created: {}", file_path.display()),
        Err(e) => eprintln!("Failed to write {}: {e}", file_path.display()),
    }
}

fn package_line(package: Option<&str>) -> String {
    package
        .map(|p| format!("package {}\n\n", p))
        .unwrap_or_default()
}

fn generate_class(name: &str, package: Option<&str>) -> String {
    format!("{}class {}", package_line(package), name)
}

fn generate_data_class(name: &str, package: Option<&str>) -> String {
    format!("{}data class {}\n", package_line(package), name)
}

fn generate_interface(name: &str, package: Option<&str>) -> String {
    format!("{}interface {}", package_line(package), name)
}

fn generate_object(name: &str, package: Option<&str>) -> String {
    format!("{}object {}", package_line(package), name)
}

fn generate_sealed_class(name: &str, package: Option<&str>) -> String {
    format!("{}sealed class {}", package_line(package), name)
}

fn generate_enum_class(name: &str, package: Option<&str>) -> String {
    format!("{}enum class {}", package_line(package), name)
}

fn generate_activity(name: &str, package: Option<&str>) -> String {
    format!(
        r#"{}import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent

class {} : ComponentActivity() {{
    override fun onCreate(savedInstanceState: Bundle?) {{
        super.onCreate(savedInstanceState)
        setContent {{
            // TODO: content
        }}
    }}
}}
"#,
        package_line(package),
        name
    )
}

fn generate_composable(name: &str, package: Option<&str>) -> String {
    format!(
        r#"{}import androidx.compose.runtime.Composable

@Composable
fun {}() {{
    // TODO: content
}}
"#,
        package_line(package),
        name
    )
}

fn generate_viewmodel(name: &str, package: Option<&str>) -> String {
    format!(
        r#"{}import androidx.lifecycle.ViewModel

class {} : ViewModel() {{
    // TODO: business logic
}}
"#,
        package_line(package),
        name
    )
}

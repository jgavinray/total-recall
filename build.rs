use std::path::Path;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    let sqlite_vec_dir = Path::new("vendor").join("sqlite-vec");

    if !sqlite_vec_dir.exists() {
        println!("cargo:warning=sqlite-vec not found, preparing for manual installation");
        println!("cargo:warning=Please run: git clone https://github.com/asg017/sqlite-vec vendor/sqlite-vec");
        return;
    }

    let sqlite_vec_c_dir = find_sqlite_vec_c_dir(&sqlite_vec_dir);

    if let Some(c_dir) = sqlite_vec_c_dir {
        println!("cargo:warning=Building sqlite-vec from {}", c_dir);

        let status = Command::new("make").args(["-C", c_dir.as_str()]).status();

        match status {
            Ok(s) if s.success() => {
                println!("cargo:warning=sqlite-vec built successfully");
            }
            Ok(s) => {
                eprintln!("cargo:warning=sqlite-vec build failed with status: {:?}", s);
            }
            Err(e) => {
                eprintln!("cargo:warning=Failed to run make: {}", e);
            }
        }
    } else {
        println!("cargo:warning=Could not find C directory in sqlite-vec");
    }
}

fn find_sqlite_vec_c_dir(base: &Path) -> Option<String> {
    for entry in std::fs::read_dir(base).ok()? {
        let entry = entry.ok()?;
        let path = entry.path();

        if path.is_dir() {
            let c_path = path.join("c");
            if c_path.exists() && c_path.join("CMakeLists.txt").exists() {
                return Some(c_path.to_string_lossy().to_string());
            }
        }
    }

    let c_path = base.join("c");
    if c_path.exists() && c_path.join("CMakeLists.txt").exists() {
        return Some(c_path.to_string_lossy().to_string());
    }

    None
}

fn main() {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
    let output = std::process::Command::new(&shell)
        .args(["-ilc", "echo $PATH"])
        .output()
        .unwrap();
    println!("PATH from {}: {}", shell, String::from_utf8_lossy(&output.stdout));
}

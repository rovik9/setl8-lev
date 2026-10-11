use std::io::Write;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut out = String::new();
    let code = setl8_verify::cli::run(&args, &|p| std::fs::read(p), &mut out);
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    let _ = lock.write_all(out.as_bytes());
    std::process::exit(code);
}

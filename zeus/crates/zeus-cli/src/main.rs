fn main() {
    let code = match zeus_cli::run(std::env::args().skip(1).collect()) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("zeus: {error}");
            error.exit_code()
        }
    };
    std::process::exit(code);
}

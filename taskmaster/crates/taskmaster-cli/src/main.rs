fn main() {
    let code = match taskmaster_cli::run(std::env::args().skip(1).collect()) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("taskmaster: {error}");
            error.exit_code()
        }
    };
    std::process::exit(code);
}

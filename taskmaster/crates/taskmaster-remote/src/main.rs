fn main() {
    let executable = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("taskmaster-remote: cannot resolve current executable: {error}");
            std::process::exit(taskmaster_remote::EXIT_FAILURE);
        }
    };
    let mut stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let mut stderr = std::io::stderr();
    let code = taskmaster_remote::execute(
        std::env::args().skip(1),
        &executable,
        &mut stdin,
        &mut stdout,
        &mut stderr,
    );
    if code != taskmaster_remote::EXIT_OK {
        std::process::exit(code);
    }
}

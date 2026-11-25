use std::{io, process::ExitCode};
fn main() -> ExitCode {
    match brrdctl::run(
        std::env::args_os(),
        || brrd_core::SystemReader::default().one_shot(),
        &mut io::stdout().lock(),
        &mut io::stderr().lock(),
    ) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("brrdctl: {error}");
            ExitCode::FAILURE
        }
    }
}

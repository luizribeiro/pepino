use std::{
    io::{self, BufRead, Write},
    path::PathBuf,
};

use nix::sys::termios::{LocalFlags, SetArg, Termios, tcgetattr, tcsetattr};
use ppn::{
    error::{CliError, ErrorKind},
    input::read_text,
    terminal::Context,
};

#[derive(clap::Args)]
pub(super) struct CodeInput {
    /// Read the secret code from stdin (use - or omit), or supply a code visible in ps.
    #[arg(value_name = "CODE")]
    code: Option<String>,
    /// Read the secret code from a file.
    #[arg(long, value_name = "PATH", conflicts_with = "code")]
    from_file: Option<PathBuf>,
}

impl CodeInput {
    pub(super) fn read(self, context: &Context<'_>) -> Result<String, CliError> {
        let source = self.code.as_deref();
        if self.from_file.is_none()
            && matches!(source, None | Some("-"))
            && context.terminal.stdin_is_tty
        {
            return prompt().map_err(|error| {
                CliError::new(
                    ErrorKind::Failure,
                    format!("Could not read the code: {error}"),
                )
            });
        }
        let code = self
            .code
            .or_else(|| self.from_file.is_none().then(|| "-".into()));
        read_text(code, self.from_file.as_deref())
    }
}

struct RestoreEcho<'a>(&'a io::Stdin, Termios);

impl Drop for RestoreEcho<'_> {
    fn drop(&mut self) {
        let _ = tcsetattr(self.0, SetArg::TCSANOW, &self.1);
    }
}

fn prompt() -> io::Result<String> {
    let stdin = io::stdin();
    let original = tcgetattr(&stdin)?;
    let mut hidden = original.clone();
    hidden
        .local_flags
        .remove(LocalFlags::ECHO | LocalFlags::ECHONL);
    tcsetattr(&stdin, SetArg::TCSANOW, &hidden)?;
    let restore = RestoreEcho(&stdin, original);
    let mut stderr = io::stderr().lock();
    write!(stderr, "Paste the secret device code: ")?;
    stderr.flush()?;
    let mut code = String::new();
    let result = stdin.lock().read_line(&mut code);
    drop(restore);
    writeln!(stderr)?;
    result?;
    Ok(code)
}

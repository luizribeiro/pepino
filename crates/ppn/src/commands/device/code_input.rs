use std::{
    io::{self, Read, Write},
    path::PathBuf,
};

use nix::sys::termios::{
    LocalFlags, SetArg, SpecialCharacterIndices, Termios, tcgetattr, tcsetattr,
};
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
        .remove(LocalFlags::ECHO | LocalFlags::ECHONL | LocalFlags::ICANON | LocalFlags::ISIG);
    hidden.control_chars[SpecialCharacterIndices::VMIN as usize] = 1;
    hidden.control_chars[SpecialCharacterIndices::VTIME as usize] = 0;
    tcsetattr(&stdin, SetArg::TCSANOW, &hidden)?;
    let restore = RestoreEcho(&stdin, original);
    let mut stderr = io::stderr().lock();
    write!(stderr, "Paste the secret device code: ")?;
    stderr.flush()?;
    let result = read_secret_line(stdin.lock());
    drop(restore);
    writeln!(stderr)?;
    result
}

fn read_secret_line(mut input: impl Read) -> io::Result<String> {
    use ppn::device_code::MAX_CODE_TEXT;
    let mut code = Vec::new();
    let mut byte = [0];
    while input.read(&mut byte)? != 0 {
        match byte[0] {
            b'\n' | b'\r' | 4 => break,
            3 => {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "device code entry cancelled",
                ));
            }
            8 | 127 => {
                code.pop();
            }
            _ if code.len() >= MAX_CODE_TEXT => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "device code is too long",
                ));
            }
            other => code.push(other),
        }
    }
    String::from_utf8(code).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

#[cfg(test)]
#[test]
fn accepts_long_pastes_and_terminal_editing() {
    let paste = format!("PPN-GRANT-{}X\x7fF\n", "B".repeat(6000));
    let code = read_secret_line(paste.as_bytes()).unwrap();
    assert_eq!(code, format!("PPN-GRANT-{}F", "B".repeat(6000)));
    assert_eq!(
        read_secret_line(&b"PPN-GRANT-\x03"[..]).unwrap_err().kind(),
        io::ErrorKind::Interrupted,
    );
}

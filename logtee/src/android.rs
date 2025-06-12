// Implementation based on piping logcat output to the log file.

use std::{
    io::{self, BufRead, BufReader, Write},
    panic,
    path::Path,
    process::{self, Child, Command},
    thread::{self, JoinHandle},
};

use regex::bytes::Regex;

use super::{create_file, RotateOptions};

pub(super) struct Inner {
    process: Child,
    handle: JoinHandle<io::Result<()>>,
}

impl Inner {
    pub fn new(path: &Path, options: RotateOptions) -> io::Result<Self> {
        let mut file = create_file(path, options);
        let (pipe_reader, pipe_writer) = io::pipe()?;

        let header_regex = Regex::new(r"^\-+\s+beginning of (main|system|crash)").unwrap();

        let handle = thread::spawn(move || {
            let mut pipe_reader = BufReader::new(pipe_reader);

            // Skip the buffer header
            let mut line = Vec::new();
            pipe_reader.read_until(b'\n', &mut line)?;

            if !header_regex.is_match(&line) {
                file.write_all(&line)?;
            }

            drop(line);

            loop {
                let chunk = pipe_reader.fill_buf()?;
                let n = chunk.len();

                if n > 0 {
                    file.write_all(chunk)?;
                    pipe_reader.consume(n);
                } else {
                    return Ok(());
                }
            }
        });

        let process = Command::new("logcat")
            .arg("--pid")
            .arg(process::id().to_string())
            .arg("-vtime")
            .arg("-vyear")
            .stdout(pipe_writer)
            .spawn()?;

        Ok(Self { process, handle })
    }

    pub fn close(mut self) -> io::Result<()> {
        self.process.kill()?;
        self.process.wait()?;

        match self.handle.join() {
            Ok(result) => result,
            Err(payload) => panic::resume_unwind(payload),
        }
    }
}
